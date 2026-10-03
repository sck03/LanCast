//! Integer handles are validated under a registry lock; concurrent close cannot free an in-flight call.
use crate::runtime::Engine;
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
static ENGINES: OnceLock<Mutex<HashMap<u64, Arc<Engine>>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
fn engines() -> &'static Mutex<HashMap<u64, Arc<Engine>>> {
    ENGINES.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn create() -> u64 {
    catch_unwind(|| {
        let Ok(engine) = Engine::new() else { return 0 };
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        engines().lock().unwrap().insert(id, Arc::new(engine));
        id
    })
    .unwrap_or(0)
}
pub fn command(handle: u64, text: &str) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let engine = engines().lock().unwrap().get(&handle).cloned();
        let Some(engine) = engine else { return -1 };
        if text.len() > crate::protocol::MAX_FRAME {
            return -2;
        }
        match crate::protocol::parse_command(text)
            .ok()
            .and_then(|v| engine.command(v).ok())
        {
            Some(_) => 0,
            None => -2,
        }
    }))
    .unwrap_or(-3)
}
pub fn poll(handle: u64) -> Option<String> {
    let engine = engines().lock().ok()?.get(&handle).cloned()?;
    engine.poll()
}
pub fn destroy(handle: u64) {
    let engine = engines().lock().ok().and_then(|mut e| e.remove(&handle));
    if let Some(engine) = engine {
        engine.close();
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn lancast_create() -> u64 {
    create()
}
#[unsafe(no_mangle)]
pub extern "C" fn lancast_abi_version() -> u32 {
    2
}
#[repr(C)]
pub struct Config {
    pub size: u32,
    pub abi_version: u32,
    pub flags: u64,
}
/// # Safety
/// config points to readable aligned storage of at least its declared size, for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lancast_create_v2(config: *const Config) -> u64 {
    if config.is_null() {
        return 0;
    }
    // Read the common header before accessing fields that may be absent in an older caller.
    let size = unsafe { std::ptr::addr_of!((*config).size).read() };
    if size < std::mem::size_of::<Config>() as u32 {
        return 0;
    }
    let config = unsafe { &*config };
    if config.abi_version != 2 || config.flags != 0 {
        return 0;
    }
    create()
}
#[unsafe(no_mangle)]
pub extern "C" fn lancast_shutdown(handle: u64) -> i32 {
    catch_unwind(|| {
        let engine = engines().lock().unwrap().get(&handle).cloned();
        if let Some(engine) = engine {
            engine.shutdown();
            0
        } else {
            -1
        }
    })
    .unwrap_or(-3)
}
/// # Safety
/// data must point to len readable bytes. No buffer is retained after return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lancast_write_ts(handle: u64, data: *const u8, len: usize) -> i32 {
    if data.is_null() || len == 0 || len > 65_424 || !len.is_multiple_of(188) {
        return -2;
    }
    #[cfg(feature = "sender")]
    {
        catch_unwind(|| {
            let engine = engines().lock().unwrap().get(&handle).cloned();
            let Some(engine) = engine else {
                return -1;
            };
            let bytes = unsafe { std::slice::from_raw_parts(data, len) };
            if engine.write_ts(bytes).is_ok() {
                0
            } else {
                -2
            }
        })
        .unwrap_or(-3)
    }
    #[cfg(not(feature = "sender"))]
    {
        let _ = handle;
        -4
    }
}
/// # Safety
/// `data` must point to `len` readable bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lancast_command(handle: u64, data: *const u8, len: usize) -> i32 {
    if data.is_null() || len > crate::protocol::MAX_FRAME {
        return -2;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    match std::str::from_utf8(bytes) {
        Ok(s) => command(handle, s),
        Err(_) => -2,
    }
}
#[repr(C)]
pub struct Buffer {
    pub data: *mut u8,
    pub len: usize,
}
#[unsafe(no_mangle)]
pub extern "C" fn lancast_poll(handle: u64) -> Buffer {
    catch_unwind(|| {
        if let Some(text) = poll(handle) {
            let mut bytes = text.into_bytes().into_boxed_slice();
            let result = Buffer {
                data: bytes.as_mut_ptr(),
                len: bytes.len(),
            };
            std::mem::forget(bytes);
            result
        } else {
            Buffer {
                data: std::ptr::null_mut(),
                len: 0,
            }
        }
    })
    .unwrap_or(Buffer {
        data: std::ptr::null_mut(),
        len: 0,
    })
}
/// # Safety
/// Pass exactly a non-freed buffer returned by lancast_poll, once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lancast_free_buffer(buffer: Buffer) {
    if !buffer.data.is_null() {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                buffer.data,
                buffer.len,
            )));
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn lancast_destroy(handle: u64) {
    let _ = catch_unwind(|| destroy(handle));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_handles_and_shutdown_are_safe() {
        assert_eq!(command(u64::MAX, "{}"), -1);
        let id = create();
        assert_ne!(id, 0);
        assert_eq!(command(id, r#"{"op":"scan","op":"stop"}"#), -2);
        assert_eq!(lancast_shutdown(id), 0);
        destroy(id);
        destroy(id);
        assert_eq!(command(id, r#"{"op":"scan"}"#), -1);
    }
}
