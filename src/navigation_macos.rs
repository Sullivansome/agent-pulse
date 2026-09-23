//! Thin AppKit adapter. No titles, transcripts, shell commands, or input injection.
use super::Origin;
use anyhow::{Result, ensure};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;
type Id = *mut objc::runtime::Object;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

fn process(pid: u32) -> Option<libc::proc_bsdinfo> {
    crate::process::bsd_info(pid).ok().flatten()
}
unsafe fn string(value: Id) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let utf8: *const std::ffi::c_char = unsafe { msg_send![value, UTF8String] };
    (!utf8.is_null()).then(|| unsafe { CStr::from_ptr(utf8).to_string_lossy().into_owned() })
}
pub fn capture(provider: crate::model::Provider) -> Option<Origin> {
    let mut pid = std::process::id();
    for _ in 0..32 {
        let info = process(pid)?;
        let name: String = info
            .pbi_comm
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8 as char)
            .collect();
        if name == provider.key() || name.starts_with(&format!("{}-", provider.key())) {
            return for_process(pid);
        }
        if info.pbi_ppid <= 1 || info.pbi_ppid == pid {
            return None;
        }
        pid = info.pbi_ppid;
    }
    None
}
fn ancestor<T>(
    mut pid: u32,
    mut parent: impl FnMut(u32) -> Option<u32>,
    mut application: impl FnMut(u32) -> Option<T>,
) -> Option<T> {
    for _ in 0..32 {
        if pid <= 1 {
            break;
        }
        if let Some(app) = application(pid) {
            return Some(app);
        }
        let next = parent(pid)?;
        if next == pid {
            break;
        }
        pid = next;
    }
    None
}

pub fn for_process(pid: u32) -> Option<Origin> {
    unsafe {
        let pool: Id = msg_send![class!(NSAutoreleasePool), new];
        let result = ancestor(pid, crate::process::parent_pid, |pid| {
            let app: Id = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid as i32];
            if !app.is_null() {
                let policy: isize = msg_send![app, activationPolicy];
                let bundle: Id = msg_send![app, bundleIdentifier];
                // Ignore Electron/helper processes and use their owning GUI app.
                if policy == 0
                    && let Some(bundle_id) = string(bundle)
                {
                    // Require full identity for the actual GUI app, never
                    // for intervening system-owned processes such as login.
                    let info = process(pid)?;
                    let name: Id = msg_send![app, localizedName];
                    return Some(Origin {
                        pid,
                        started_sec: info.pbi_start_tvsec,
                        started_usec: info.pbi_start_tvusec,
                        name: string(name).unwrap_or_else(|| "Terminal".into()),
                        bundle_id,
                    });
                }
            }
            None
        });
        let _: () = msg_send![pool, drain];
        result
    }
}

pub fn codex_available() -> bool {
    unsafe {
        let pool: Id = msg_send![class!(NSAutoreleasePool), new];
        let text: Id = msg_send![class!(NSString), stringWithUTF8String: c"codex://threads/00000000-0000-0000-0000-000000000000".as_ptr()];
        let url: Id = msg_send![class!(NSURL), URLWithString: text];
        let workspace: Id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let handler: Id = msg_send![workspace, URLForApplicationToOpenURL: url];
        let available = !handler.is_null();
        let _: () = msg_send![pool, drain];
        available
    }
}

pub fn open(origin: &Origin) -> Result<()> {
    let current = for_process(origin.pid);
    ensure!(
        current.as_ref().is_some_and(|c| c.pid == origin.pid
            && c.started_sec == origin.started_sec
            && c.started_usec == origin.started_usec
            && c.bundle_id == origin.bundle_id),
        "Original window is no longer available"
    );
    unsafe {
        let pool: Id = msg_send![class!(NSAutoreleasePool), new];
        let app: Id = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: origin.pid as i32];
        let activated: objc::runtime::BOOL = msg_send![app, activateWithOptions: 2usize];
        let _: () = msg_send![pool, drain];
        ensure!(
            activated == objc::runtime::YES,
            "Could not activate the original application"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_discovery_crosses_non_application_helpers() {
        // CLI -> shell -> system login -> terminal app. Only the application
        // supplies an Origin; intermediate nodes need a parent ID alone.
        let result = ancestor(
            40,
            |pid| match pid {
                40 => Some(30),
                30 => Some(20),
                20 => Some(10),
                _ => None,
            },
            |pid| (pid == 10).then_some("Ghostty"),
        );
        assert_eq!(result, Some("Ghostty"));
        assert_eq!(ancestor(40, |_| None, |_| None::<()>), None);
        assert_eq!(ancestor(40, Some, |_| None::<()>), None);
        assert_eq!(
            ancestor(
                40,
                |pid| Some(if pid == 40 { 30 } else { 40 }),
                |_| None::<()>
            ),
            None
        );
    }

    #[test]
    #[ignore = "requires an explicitly supplied live Ghostty descendant PID"]
    fn live_ghostty_origin_and_activation() {
        let pid = std::env::var("ISLAND_TEST_GHOSTTY_CHILD_PID")
            .unwrap()
            .parse()
            .unwrap();
        let origin = for_process(pid).expect("live terminal origin");
        assert_eq!(origin.bundle_id, "com.mitchellh.ghostty");
        let mut wrong = origin.clone();
        wrong.started_usec += 1;
        assert!(
            open(&wrong).is_err(),
            "reused process identity must still be rejected"
        );
        if std::env::var_os("ISLAND_TEST_ACTIVATE_GHOSTTY").is_some() {
            open(&origin).unwrap();
        }
        println!("Resolved {} via its verified process identity", origin.name);
    }
}
