use jni::objects::{JObject, JString, JValue};
use jni::JavaVM;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::models::{MediaItem, VideoQuality};

static ANDROID_HANDLES: OnceLock<(usize, usize)> = OnceLock::new();
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();
static BACK_PRESSED: AtomicBool = AtomicBool::new(false);
static MENU_PRESSED: AtomicBool = AtomicBool::new(false);

pub fn init(vm_ptr: *mut c_void, activity_ptr: *mut c_void) {
    let _ = ANDROID_HANDLES.set((vm_ptr as usize, activity_ptr as usize));
}

pub fn set_egui_ctx(ctx: egui::Context) {
    let _ = EGUI_CTX.set(ctx);
}

pub fn take_back_pressed() -> bool {
    BACK_PRESSED.swap(false, Ordering::SeqCst)
}

pub fn take_menu_pressed() -> bool {
    MENU_PRESSED.swap(false, Ordering::SeqCst)
}

#[no_mangle]
pub extern "system" fn Java_com_burbsklopek_swayrust_SwayRustActivity_nativeOnBackPressed(
    _env: jni::JNIEnv,
    _obj: JObject,
) {
    BACK_PRESSED.store(true, Ordering::SeqCst);
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
}

#[no_mangle]
pub extern "system" fn Java_com_burbsklopek_swayrust_SwayRustActivity_nativeOnMenuPressed(
    _env: jni::JNIEnv,
    _obj: JObject,
) {
    MENU_PRESSED.store(true, Ordering::SeqCst);
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
}

fn with_activity<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut jni::JNIEnv, &JObject) -> Option<R>,
{
    let &(vm_ptr, act_ptr) = ANDROID_HANDLES.get()?;
    if vm_ptr == 0 || act_ptr == 0 {
        return None;
    }
    unsafe {
        let vm = JavaVM::from_raw(vm_ptr as *mut _).ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let activity = JObject::from_raw(act_ptr as *mut _);
        let res = f(&mut env, &activity);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        let _ = activity.into_raw();
        res
    }
}

pub fn get_files_dir() -> String {
    with_activity(|env, act| {
        let val = env
            .call_method(act, "rustGetFilesDir", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let jstr = JString::from(val);
        let s: String = env.get_string(&jstr).ok()?.into();
        Some(s)
    })
    .unwrap_or_else(|| "/data/data/com.burbsklopek.swayrust/files".to_string())
}

pub fn get_cache_dir() -> String {
    with_activity(|env, act| {
        let val = env
            .call_method(act, "rustGetCacheDir", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let jstr = JString::from(val);
        let s: String = env.get_string(&jstr).ok()?.into();
        Some(s)
    })
    .unwrap_or_else(|| "/data/data/com.burbsklopek.swayrust/cache".to_string())
}

pub fn install_apk(apk_path: &str) -> String {
    with_activity(|env, act| {
        let j_path = env.new_string(apk_path).ok()?;
        let val = env
            .call_method(
                act,
                "rustInstallApk",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&JObject::from(j_path))],
            )
            .ok()?
            .l()
            .ok()?;
        let jstr = JString::from(val);
        let s: String = env.get_string(&jstr).ok()?.into();
        Some(s)
    })
    .unwrap_or_else(|| "ERR:JNI unavailable".to_string())
}

pub fn get_lan_ip() -> Option<String> {
    with_activity(|env, act| {
        let val = env
            .call_method(act, "rustGetLanIp", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let jstr = JString::from(val);
        let s: String = env.get_string(&jstr).ok()?.into();
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    })
}

pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for b in input.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

pub fn build_trailer_url(item: &MediaItem, resolved_yt_id: Option<&str>) -> String {
    if let Some(yt) = resolved_yt_id
        .map(str::trim)
        .filter(|s| s.len() == 11 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    {
        format!("https://www.youtube.com/watch?v={yt}")
    } else {
        let yr: String = item
            .year
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let q = if yr.len() == 4 {
            format!("{} {} official trailer", item.title, yr)
        } else {
            format!("{} official trailer", item.title)
        };
        format!("https://www.youtube.com/tv#/search?q={}", url_encode(&q))
    }
}

pub fn launch_trailer(item: &MediaItem, quality: VideoQuality, resolved_yt_id: Option<&str>) -> bool {
    let url = build_trailer_url(item, resolved_yt_id);
    let height_px = quality.height_px();
    let vq = quality.youtube_vq();
    with_activity(|env, act| {
        let j_url = env.new_string(&url).ok()?;
        let j_vq = env.new_string(vq).ok()?;
        let ok = env
            .call_method(
                act,
                "rustLaunchTrailer",
                "(Ljava/lang/String;ILjava/lang/String;)Z",
                &[
                    JValue::Object(&JObject::from(j_url)),
                    JValue::Int(height_px),
                    JValue::Object(&JObject::from(j_vq)),
                ],
            )
            .ok()?
            .z()
            .ok()?;
        Some(ok)
    })
    .unwrap_or(false)
}

pub fn exit_app() {
    let _ = with_activity(|env, act| {
        let _ = env.call_method(act, "rustExitApp", "()V", &[]);
        Some(())
    });
}
