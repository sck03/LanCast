//! JNI owns copied encoded buffers only for the duration of a callback.
//! Java returns false on backpressure; exceptions fail the session closed.
use crate::engine::{Event, Output, Running};
use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JByteArray, JObject, JString, JValue},
    sys::{jboolean, jlong},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};

struct JavaOutput {
    vm: JavaVM,
    object: GlobalRef,
}
impl Output for JavaOutput {
    fn emit(&self, event: Event) -> bool {
        let Ok(mut env) = self.vm.attach_current_thread() else {
            return false;
        };
        let result=env.with_local_frame(16,|env|->jni::errors::Result<bool>{
            let obj=self.object.as_obj();
            let event_result=match event{
                Event::Ready{port,name,device_id,public_key,features}=>{
                    let text=env.new_string(serde_json::json!({"port":port,"name":name,"deviceId":device_id,"publicKey":public_key,"features":format!("0x{:X},0x{:X}",features as u32,features>>32)}).to_string())?;
                    env.call_method(obj,"onEvent","(JILjava/lang/String;)V",&[JValue::Long(0),JValue::Int(1),JValue::Object(&text)])?
                },
                Event::Request{session,name,peer}=>{let text=env.new_string(serde_json::json!({"name":name,"peer":peer}).to_string())?;env.call_method(obj,"onEvent","(JILjava/lang/String;)V",&[JValue::Long(session as i64),JValue::Int(2),JValue::Object(&text)])?},
                Event::Closed{session,reason}=>{let text=env.new_string(reason)?;env.call_method(obj,"onEvent","(JILjava/lang/String;)V",&[JValue::Long(session as i64),JValue::Int(3),JValue::Object(&text)])?},
                Event::Error(reason)=>{let text=env.new_string(reason)?;env.call_method(obj,"onEvent","(JILjava/lang/String;)V",&[JValue::Long(0),JValue::Int(4),JValue::Object(&text)])?},
                Event::VideoConfig{session,config}=>{
                    let sps=env.byte_array_from_slice(&config.sps)?;let pps=env.byte_array_from_slice(&config.pps)?;
                    env.call_method(obj,"onVideoConfig","(JII[B[B)Z",&[JValue::Long(session as i64),JValue::Int(config.width as i32),JValue::Int(config.height as i32),JValue::Object(&sps),JValue::Object(&pps)])?
                },
                Event::Video{session,pts,key,data}=>{let bytes=env.byte_array_from_slice(&data)?;env.call_method(obj,"onVideo","(JJZ[B)Z",&[JValue::Long(session as i64),JValue::Long(pts),JValue::Bool(key as u8),JValue::Object(&bytes)])?},
                Event::AudioConfig{session,codec,rate,channels,spf}=>env.call_method(obj,"onAudioConfig","(JIIII)Z",&[JValue::Long(session as i64),JValue::Int(codec as i32),JValue::Int(rate as i32),JValue::Int(channels as i32),JValue::Int(spf as i32)])?,
                Event::Audio{session,pts,data}=>{let bytes=env.byte_array_from_slice(&data)?;env.call_method(obj,"onAudio","(JJ[B)Z",&[JValue::Long(session as i64),JValue::Long(pts),JValue::Object(&bytes)])?},
            };
            Ok(match event_result{jni::objects::JValueOwned::Bool(v)=>v!=0,_=>true})
        });
        if env.exception_check().unwrap_or(true) {
            let _ = env.exception_clear();
            return false;
        }
        result.unwrap_or(false)
    }
}
static NEXT: AtomicI64 = AtomicI64::new(1);
static ENGINES: OnceLock<Mutex<HashMap<i64, Running>>> = OnceLock::new();
fn engines() -> &'static Mutex<HashMap<i64, Running>> {
    ENGINES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_airplay_NativeEngine_startNative(
    mut env: JNIEnv,
    object: JObject,
    address: JString,
    name: JString,
    seed: JByteArray,
    pin: JString,
) -> jlong {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Option<i64> {
        let address: String = env.get_string(&address).ok()?.into();
        let name: String = env.get_string(&name).ok()?.into();
        let pin: String = env.get_string(&pin).ok()?.into();
        let seed: [u8; 32] = env.convert_byte_array(seed).ok()?.try_into().ok()?;
        if name.is_empty()
            || name.len() > 120
            || name.chars().any(char::is_control)
            || pin.len() != 8
        {
            return None;
        }
        let mut digits = [0; 8];
        for (i, c) in pin.bytes().enumerate() {
            if !c.is_ascii_digit() {
                return None;
            }
            digits[i] = c - b'0';
        }
        let output = Arc::new(JavaOutput {
            vm: env.get_java_vm().ok()?,
            object: env.new_global_ref(object).ok()?,
        });
        let mut entries = engines().lock().ok()?;
        if !entries.is_empty() {
            return None;
        }
        let running = Running::start(address.parse().ok()?, name, seed, digits, output).ok()?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        entries.insert(id, running);
        Some(id)
    }))
    .ok()
    .flatten()
    .unwrap_or(0)
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_airplay_NativeEngine_approveNative(
    _env: JNIEnv,
    _object: JObject,
    handle: jlong,
    session: jlong,
    accept: jboolean,
) {
    let _ = std::panic::catch_unwind(|| {
        if let Some(e) = engines().lock().unwrap().get(&handle) {
            e.host.approve(session as u64, accept != 0);
        }
    });
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_airplay_NativeEngine_stopSessionNative(
    _env: JNIEnv,
    _object: JObject,
    handle: jlong,
    session: jlong,
) {
    let _ = std::panic::catch_unwind(|| {
        if let Some(e) = engines().lock().unwrap().get(&handle) {
            e.host.stop_session(session as u64, "USER_STOP");
        }
    });
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_lancast_airplay_NativeEngine_stopNative(
    _env: JNIEnv,
    _object: JObject,
    handle: jlong,
) {
    let _ = std::panic::catch_unwind(|| {
        let running = engines().lock().unwrap().remove(&handle);
        if let Some(running) = running {
            running.stop();
        }
    });
}
