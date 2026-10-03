use jni::{
    JNIEnv,
    objects::{JClass, JString},
    sys::{jint, jlong, jstring},
};
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_control_NativeCore_create(_: JNIEnv, _: JClass) -> jlong {
    crate::ffi::create() as jlong
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
pub extern "system" fn Java_dev_lancast_control_NativeCore_destroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    crate::ffi::destroy(handle as u64);
}
