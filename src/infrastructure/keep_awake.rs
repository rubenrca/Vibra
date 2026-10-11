//! A macOS idle-sleep assertion owned by the workspace, never persisted.

use anyhow::{Result, bail};

#[derive(Default)]
pub struct KeepAwake {
    assertion_id: Option<u32>,
}

impl KeepAwake {
    pub fn is_enabled(&self) -> bool {
        self.assertion_id.is_some()
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        match (enabled, self.assertion_id) {
            (true, None) => {
                let mut id = 0;
                // The bridge writes an assertion ID into this valid stack pointer.
                let result = unsafe { vibra_keep_awake_enable(&mut id) };
                if result != 0 {
                    bail!("Could not enable Keep Awake (macOS error 0x{result:08x}).");
                }
                self.assertion_id = Some(id);
            }
            (false, Some(id)) => {
                // We own this assertion until macOS successfully releases it.
                let result = unsafe { vibra_keep_awake_disable(id) };
                if result != 0 {
                    bail!("Could not disable Keep Awake (macOS error 0x{result:08x}).");
                }
                self.assertion_id = None;
            }
            _ => {}
        }
        Ok(())
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if let Err(error) = self.set_enabled(false) {
            eprintln!("{error}");
        }
    }
}

unsafe extern "C" {
    fn vibra_keep_awake_enable(assertion_id: *mut u32) -> i32;
    fn vibra_keep_awake_disable(assertion_id: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;

    unsafe extern "C" {
        fn IOPMAssertionCopyProperties(assertion_id: u32) -> *const c_void;
        fn CFRelease(value: *const c_void);
    }

    fn assertion_exists(id: u32) -> bool {
        // CopyProperties returns a retained dictionary, or null after release.
        unsafe {
            let properties = IOPMAssertionCopyProperties(id);
            if properties.is_null() {
                return false;
            }
            CFRelease(properties);
            true
        }
    }

    #[test]
    fn macos_assertion_is_released_when_disabled_or_dropped() {
        let mut keep_awake = KeepAwake::default();
        assert!(!keep_awake.is_enabled());
        keep_awake.set_enabled(false).unwrap();

        keep_awake.set_enabled(true).unwrap();
        let id = keep_awake.assertion_id.unwrap();
        assert!(assertion_exists(id));
        keep_awake.set_enabled(true).unwrap();
        assert_eq!(keep_awake.assertion_id, Some(id));

        keep_awake.set_enabled(false).unwrap();
        assert!(!keep_awake.is_enabled());
        assert!(!assertion_exists(id));

        keep_awake.set_enabled(true).unwrap();
        let id = keep_awake.assertion_id.unwrap();
        assert!(assertion_exists(id));
        drop(keep_awake);
        assert!(!assertion_exists(id));
    }
}
