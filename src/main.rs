use anyhow::Context;
use libc::{c_int, fcntl, FD_CLOEXEC, F_GETFD, F_SETFD};
use log::{debug, info};
use nix::{
    fcntl::readlink,
    sys::stat::fstat,
    unistd::{lseek, Whence},
};
use rusb::{constants::LIBUSB_OPTION_NO_DEVICE_DISCOVERY, UsbContext};
use sendfd::{RecvWithFd, SendWithFd};
use std::{
    env, io,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::net::UnixDatagram,
    },
    path::PathBuf,
    process::{Command, ExitStatus},
    ptr::null_mut,
    str,
    time::Duration,
};

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct UsbSerial {
    number: String,
    path: PathBuf,
}

fn init_libusb_device_serial(usb_fd: c_int) -> anyhow::Result<UsbSerial> {
    debug!("calling libusb_set_option");
    unsafe { rusb::ffi::libusb_set_option(null_mut(), LIBUSB_OPTION_NO_DEVICE_DISCOVERY) };

    lseek(usb_fd, 0, Whence::SeekSet).with_context(|| format!("error seeking fd: {}", usb_fd))?;

    let ctx = rusb::Context::new().context("libusb_init error")?;

    debug!("opening device from {}", usb_fd);
    let usb_handle = unsafe {
        ctx.open_device_with_fd(usb_fd)
            .context("error opening device")
    }?;

    debug!("getting device from handle");
    let usb_dev = usb_handle.device();

    debug!("requesting device descriptor");
    let usb_dev_desc = usb_dev
        .device_descriptor()
        .context("error getting device descriptor")?;

    let vid = usb_dev_desc.vendor_id();
    let pid = usb_dev_desc.product_id();
    let iser = usb_dev_desc.serial_number_string_index();
    debug!(
        "device descriptor: vid={}, pid={}, iSerial={}",
        vid,
        pid,
        iser.unwrap_or(0)
    );

    let timeout = Duration::from_secs(1);
    let languages = usb_handle
        .read_languages(timeout)
        .context("error getting supported languages for reading string descriptors")?;

    let serial_number = usb_handle
        .read_serial_number_string(languages[0], &usb_dev_desc, timeout)
        .context("error reading serial number of the device")?;

    let st = fstat(usb_fd).context("error: could not stat TERMUX_USB_FD")?;
    let dev_path_link = format!("/sys/dev/char/{}:{}", major(st.st_rdev), minor(st.st_rdev));

    let dev_path = PathBuf::from(readlink(&PathBuf::from(&dev_path_link)).context(format!(
        "error: could not resolve symlink {}",
        &dev_path_link
    ))?);

    let mut dev_serial_path = PathBuf::from("/sys/bus/usb/devices");

    dev_serial_path.push(
        dev_path
            .file_name()
            .context("error: could not get device path")?,
    );
    dev_serial_path.push("serial");

    info!("device serial path: {}", dev_serial_path.display());

    Ok(UsbSerial {
        number: serial_number,
        path: dev_serial_path,
    })
}

pub const fn major(dev: u64) -> u64 {
    ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)
}

pub const fn minor(dev: u64) -> u64 {
    ((dev >> 12) & 0xffff_ff00) | ((dev) & 0x0000_00ff)
}

fn test_usb_with_uds() -> anyhow::Result<()> {
    let self_path = env::current_exe().context("failed to get executable path")?;
    let (sock_send, sock_recv) = UnixDatagram::pair().context("could not create socket pair")?;
    _ = clear_cloexec_flag(&sock_send);

    let usb_dev_list = get_termux_usb_list();
    println!("{:?}", usb_dev_list);

    for dev in &usb_dev_list {
        run_under_termux_usb(&dev, &self_path, sock_send.as_raw_fd())
            .context("error running termux-usb")?;

        let mut buf = vec![0; 256];
        let mut fds = vec![0; 1];
        match sock_recv.recv_with_fd(buf.as_mut_slice(), fds.as_mut_slice()) {
            Ok((_, 0)) => {
                eprintln!("received message without usb fd");
            }
            Ok((size, _)) => {
                let usb_dev_path = PathBuf::from(String::from_utf8_lossy(&buf[0..size]).as_ref());
                let usb_fd = fds[0];
                // use the received info as TERMUX_USB_DEV and TERMUX_USB_FD
                println!(
                    "received message (size={}) with fd={}: {}",
                    size,
                    usb_fd,
                    usb_dev_path.display()
                );

                let usb_serial = init_libusb_device_serial(usb_fd)?;
                println!("{:?}", usb_serial);
            }
            Err(e) => {
                eprintln!("message receive error: {}", e);
            }
        }
    }

    Ok(())
}

fn run_under_termux_usb(dev: &str, self_path: &PathBuf, sock_fd: RawFd) -> io::Result<ExitStatus> {
    let mut cmd = Command::new("termux-usb");
    cmd.arg("-e");
    cmd.arg(self_path);
    cmd.args(["-E", "-r", dev]);
    cmd.env("TERMUX_USB_DEV", dev);
    cmd.env("TERMUX_ADB_SOCK_FD", sock_fd.to_string());
    cmd.status()
}

fn clear_cloexec_flag(socket: &UnixDatagram) -> RawFd {
    let sock_fd = socket.as_raw_fd();
    unsafe {
        let flags = fcntl(sock_fd, F_GETFD);
        fcntl(sock_fd, F_SETFD, flags & !FD_CLOEXEC);
    }
    sock_fd
}

fn get_termux_usb_list() -> Vec<String> {
    if let Ok(out) = Command::new("termux-usb").arg("-l").output() {
        if let Ok(stdout) = str::from_utf8(&out.stdout) {
            if let Ok(lst) = serde_json::from_str(stdout) {
                return lst;
            }
        }
    }
    vec![]
}

fn test_usb() -> anyhow::Result<()> {
    let fd_str = env::var("TERMUX_USB_FD").context(concat!(
        "error: TERMUX_USB_FD not set, ",
        "you must run termux-usb -e ./termux-usb-test -E -r /dev/bus/usb/..."
    ))?;
    let usb_fd = fd_str
        .parse::<c_int>()
        .context("error: could not parse TERMUX_USB_FD")?;
    let usb_serial = init_libusb_device_serial(usb_fd)?;
    println!("{:?}", usb_serial);

    Ok(())
}

fn sendfd_to_adb(
    termux_usb_dev: &str,
    termux_usb_fd: &str,
    sock_send_fd: &str,
) -> anyhow::Result<()> {
    let socket = unsafe { UnixDatagram::from_raw_fd(sock_send_fd.parse()?) };
    // send termux_usb_dev and termux_usb_fd to adb-hooks
    match socket.send_with_fd(termux_usb_dev.as_bytes(), &[termux_usb_fd.parse()?]) {
        Ok(_) => {
            info!(
                "found {}, sending fd {} to parent",
                &termux_usb_dev, &termux_usb_fd
            );
        }
        Err(e) => {
            eprintln!("error sending usb fd to parent: {}", e);
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    match (
        env::var("TERMUX_USB_DEV"),
        env::var("TERMUX_USB_FD"),
        env::var("TERMUX_ADB_SOCK_FD"),
    ) {
        (Ok(termux_usb_dev), Ok(termux_usb_fd), Ok(sock_send_fd)) => {
            return sendfd_to_adb(&termux_usb_dev, &termux_usb_fd, &sock_send_fd);
        }
        _ => {}
    }

    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 && args[1] == "--test-uds" {
        return test_usb_with_uds();
    }

    test_usb()
}
