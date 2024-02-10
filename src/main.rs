use anyhow::Context;
use libc::c_int;
use log::{debug, info};
use nix::{
    fcntl::readlink,
    sys::stat::fstat,
    unistd::{lseek, Whence},
};
use rusb::{constants::LIBUSB_OPTION_NO_DEVICE_DISCOVERY, UsbContext};
use std::{env, path::PathBuf, ptr::null_mut, time::Duration};

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

fn main() -> anyhow::Result<()> {
    env_logger::init();

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
