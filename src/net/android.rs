//! Android MulticastLock JNI bridge.
//!
//! Android Wi-Fi stacks drop multicast traffic to save power. This module
//! acquires a `WifiManager.MulticastLock` so that UDP multicast discovery
//! works over Wi-Fi.

#[cfg(target_os = "android")]
use std::sync::Mutex;

#[cfg(target_os = "android")]
use jni::objects::{GlobalRef, JObject, JValue};
#[cfg(target_os = "android")]
use jni::JavaVM;

/// Application-wide multicast lock guard. Acquires on creation, releases on drop.
#[cfg(target_os = "android")]
pub struct MulticastLockGuard {
    lock: GlobalRef,
    vm: JavaVM,
}

// SAFETY: GlobalRef is Send+Sync and the JavaVM pointer is stable for the
// application lifetime.
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
unsafe impl Send for MulticastLockGuard {}
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
unsafe impl Sync for MulticastLockGuard {}

#[cfg(target_os = "android")]
impl MulticastLockGuard {
    /// Acquire an Android `WifiManager.MulticastLock`.
    ///
    /// The lock is held until this guard is dropped.
    #[allow(unsafe_code)]
    pub fn acquire() -> Result<Self, String> {
        let ctx = ndk_context::android_context();
        let vm_ptr = ctx.vm();
        if vm_ptr.is_null() {
            return Err("Android context VM is null".into());
        }

        // SAFETY: ndk_context::android_context() returns the real JNI VM
        // pointer that was set during android_main.
        let vm = unsafe { JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) }
            .map_err(|e| e.to_string())?;

        // Acquire the lock in a block so the AttachGuard is dropped before
        // we move `vm` into the struct.
        let global_lock = {
            let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;

            // Cast *mut c_void to jobject for jni 0.21.
            let ctx_obj =
                unsafe { JObject::from_raw(ctx.context() as jni::sys::jobject) };

            // 1. getSystemService(Context.WIFI_SERVICE)
            let wifi_str = env.new_string("wifi").map_err(|e| e.to_string())?;
            let wifi_service = env
                .call_method(
                    &ctx_obj,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::from(&wifi_str)],
                )
                .map_err(|e| e.to_string())?
                .l()
                .map_err(|e| e.to_string())?;

            if wifi_service.is_null() {
                return Err("getSystemService(wifi) returned null".into());
            }

            // 2. Get WifiManager.mLocks (MulticastLock field)
            let lock_obj = env
                .get_field(
                    &wifi_service,
                    "mLocks",
                    "Landroid/net/wifi/WifiManager$MulticastLock;",
                )
                .map_err(|e| e.to_string())?
                .l()
                .map_err(|e| e.to_string())?;

            if lock_obj.is_null() {
                return Err("WifiManager.mLocks is null".into());
            }

            // 3. Acquire the multicast lock
            env.call_method(&lock_obj, "acquire", "()V", &[])
                .map_err(|e| e.to_string())?;

            // 4. Promote to a global reference so it outlives this JNI call.
            env.new_global_ref(&lock_obj).map_err(|e| e.to_string())?
            // env (AttachGuard) is dropped here, releasing the borrow on vm.
        };

        log::info!("Android MulticastLock acquired");
        Ok(Self {
            lock: global_lock,
            vm,
        })
    }
}

#[cfg(target_os = "android")]
impl Drop for MulticastLockGuard {
    fn drop(&mut self) {
        let Ok(mut env) = self.vm.attach_current_thread() else {
            log::warn!("Failed to attach thread to release MulticastLock");
            return;
        };
        if let Err(e) = env.call_method(self.lock.as_obj(), "release", "()V", &[]) {
            log::warn!("Failed to release MulticastLock: {}", e);
        } else {
            log::info!("Android MulticastLock released");
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy standalone API kept for backward compatibility.
// Prefer MulticastLockGuard::acquire() instead.
// ---------------------------------------------------------------------------

/// Global lock slot for the legacy acquire/release API.
#[cfg(target_os = "android")]
static GLOBAL_LOCK: Mutex<Option<MulticastLockGuard>> = Mutex::new(None);

/// Acquire the Android multicast lock (legacy standalone API).
///
/// Returns `Ok(())` on success. On non-Android platforms this is a no-op.
/// Prefer `MulticastLockGuard::acquire()` for new code.
#[cfg(target_os = "android")]
pub fn acquire_multicast_lock() -> Result<(), String> {
    let guard = MulticastLockGuard::acquire()?;
    let mut slot = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    *slot = Some(guard);
    Ok(())
}

/// Release the Android multicast lock (legacy standalone API).
#[cfg(target_os = "android")]
pub fn release_multicast_lock() {
    let mut slot = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if slot.take().is_some() {
        // Drop releases the lock.
    }
}

// Non-Android stubs
#[cfg(not(target_os = "android"))]
pub fn acquire_multicast_lock() -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "android"))]
pub fn release_multicast_lock() {}
