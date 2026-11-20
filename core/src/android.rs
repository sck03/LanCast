use jni::{
    JNIEnv,
    objects::{JByteBuffer, JClass, JString},
    sys::{jint, jlong, jstring},
};
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_create(_: JNIEnv, _: JClass) -> jlong {
    crate::ffi::create() as jlong
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_writeTs(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    input: JByteBuffer,
) -> jint {
    let Ok(length) = env.get_direct_buffer_capacity(&input) else {
        return -2;
    };
    if length == 0 || length > 65_424 {
        return -2;
    }
    let Ok(bytes) = env.get_direct_buffer_address(&input) else {
        return -2;
    };
    unsafe { crate::ffi::lancast_write_ts(handle as u64, bytes, length) }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_command(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    input: JString,
) -> jint {
    let Ok(text) = env.get_string(&input) else {
        return -2;
    };
    crate::ffi::command(handle as u64, &text.to_string_lossy())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_shareFile(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    fd: jint,
    input: JString,
) -> jint {
    use std::os::fd::BorrowedFd;
    if fd < 0 {
        return -2;
    }
    // The Java caller holds its ParcelFileDescriptor until this synchronous call returns.
    let Ok(file) = (unsafe { BorrowedFd::borrow_raw(fd) }).try_clone_to_owned() else {
        return -2;
    };
    let Ok(text) = env.get_string(&input) else {
        return -2;
    };
    crate::ffi::share_file(handle as u64, file.into(), &text.to_string_lossy())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_poll(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    crate::ffi::poll(handle as u64)
        .and_then(|s| env.new_string(s).ok())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_waitPoll(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    timeout_ms: jint,
) -> jstring {
    crate::ffi::wait_poll(handle as u64, timeout_ms.clamp(0, 1000) as u32)
        .and_then(|s| env.new_string(s).ok())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_shutdown(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    crate::ffi::lancast_shutdown(handle as u64);
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_destroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    crate::ffi::destroy(handle as u64);
}
