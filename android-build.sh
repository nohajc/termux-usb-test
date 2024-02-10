#!/bin/bash

if [[ $(uname -o) =~ "Linux" ]]; then
    export PATH=$PATH:$HOME/Android/android-ndk-r26b/toolchains/llvm/prebuilt/linux-x86_64/bin
elif [[ $(uname -o) =~ "Darwin" ]]; then
    export PATH=$PATH:$HOME/Android/android-ndk-r26b/toolchains/llvm/prebuilt/darwin-x86_64/bin
else
    export PATH=$PATH:$HOME/Android/android-ndk-r26b/toolchains/llvm/prebuilt/windows-x86_64/bin
fi

cargo build --release --target aarch64-linux-android
