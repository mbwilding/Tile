//! The shell's half of starting Store apps such as Settings and the Microsoft Store.
//!
//! Windows starts a Store app through `sihost.exe`, which then waits for the shell to say the
//! app's window is ready. Explorer does that with the shell's `UwpWindowLifecycleManager`. With
//! no Explorer nothing answers, and a launch hangs for 45 seconds and fails with 0x80040900.
//! [`start`] creates the same manager in this process, the way Explorer does.
//!
//! This rests on Windows internals found by reading the debug symbols of Windows 11 (build
//! 26100): a CoreMessaging session, and the manager's static `Create`, which has to be called
//! from a thread that belongs to a session. If an update changes any of it, [`start`] logs the
//! failure and Store apps hang as before; nothing else is affected.

use std::{ffi::c_void, ptr, thread};

use windows::{
    Win32::{
        System::{
            Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            WinRT::{RO_INIT_MULTITHREADED, RoGetActivationFactory, RoInitialize},
        },
        UI::Shell::{ACTIVATEOPTIONS, ApplicationActivationManager, IApplicationActivationManager},
    },
    core::{GUID, HRESULT, HSTRING, IInspectable, Interface, PCWSTR},
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::{
        LibraryLoader::{GetProcAddress, LoadLibraryW},
        Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    },
    UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId},
};

/// The shell component that completes Store app launches for `sihost`.
const MANAGER_CLASS: &str =
    "Windows.Internal.Shell.UwpWindowLifecycleManagement.UwpWindowLifecycleManager";
/// Its statics interface, `IUwpWindowLifecycleManagerStatics`.
const STATICS: GUID = GUID::from_u128(0x6C69_1047_8DB8_5D6E_A382_AE35_ACDA_1DD5);
/// The statics' methods follow `IInspectable`'s six: `get_Current`, then `Create`.
const CREATE_SLOT: usize = 7;
/// `Run` in the vtable of the session `CoreUICreateEx` makes.
const RUN_SLOT: usize = 29;

type Method = extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;

/// Whether Explorer owns a taskbar window, which makes it the shell.
fn explorer_is_shell() -> bool {
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    let mut after = ptr::null_mut();
    loop {
        // SAFETY: `class` is NUL-terminated and outlives the call.
        after = unsafe { FindWindowExW(ptr::null_mut(), after, class.as_ptr(), ptr::null()) };
        if after.is_null() {
            return false;
        }
        let mut pid = 0;
        // SAFETY: `after` was just returned by the search; `pid` is a live local.
        unsafe { GetWindowThreadProcessId(after, &raw mut pid) };
        // SAFETY: a query-only handle that is closed again below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            continue;
        }
        let mut buffer = [0u16; 520];
        let mut length = buffer.len() as u32;
        // SAFETY: `buffer` has room for `length` units and `process` is open.
        let ok =
            unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &raw mut length) };
        // SAFETY: closes the handle opened above.
        unsafe { CloseHandle(process) };
        if ok != 0 {
            let path = String::from_utf16_lossy(&buffer[..length as usize]);
            let name = path.rsplit('\\').next().unwrap_or_default();
            if name.eq_ignore_ascii_case("explorer.exe") {
                return true;
            }
        }
    }
}

/// Starts the Store app support on a thread of its own, unless Explorer is the shell and already
/// does this. The thread runs for the life of the process.
pub fn start() {
    if explorer_is_shell() {
        tracing::info!("Explorer is the shell; Store apps are left to it");
        return;
    }
    crate::frame::start();
    let spawned = thread::Builder::new()
        .name("store apps".into())
        .spawn(|| match host() {
            Ok(()) => tracing::info!("the Store app host stopped"),
            Err(e) => tracing::warn!("Store apps won't start without Explorer: {e}"),
        });
    if let Err(e) = spawned {
        tracing::warn!("couldn't start the Store app host: {e}");
    }
}

/// Starts the Store app with this app user model id (`Microsoft.WindowsStore_8wekyb3d8bbwe!App`)
/// and returns its process id. `shell:AppsFolder\<id>` can't be used without Explorer: it asks
/// for a COM class that only Explorer registers, and fails with "class not registered". Call
/// this from a process with the user's normal rights, as Explorer's own is.
pub fn activate(aumid: &str) -> Result<u32, String> {
    let id: Vec<u16> = aumid.encode_utf16().chain([0]).collect();
    // SAFETY: plain COM calls; `id` is NUL-terminated and outlives the call.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)
                .map_err(|e| format!("creating the activation manager: {e}"))?;
        manager
            .ActivateApplication(PCWSTR(id.as_ptr()), PCWSTR::null(), ACTIVATEOPTIONS(0))
            .map_err(|e| format!("starting {aumid}: {e}"))
    }
}

fn check(hr: HRESULT, what: &str) -> Result<(), String> {
    hr.ok().map_err(|e| format!("{what}: {e}"))
}

/// Makes a CoreMessaging session, creates the lifecycle manager from its thread, then runs the
/// session, which returns only when the process ends.
fn host() -> Result<(), String> {
    // SAFETY: calls CoreMessaging's `CoreUICreateEx` and the manager's static `Create` through
    // their vtables, with the argument lists they have in Windows 11 build 26100. Every pointer
    // passed is a live local or one the previous call returned, and the interfaces stay alive
    // because the factory and session are kept until the function returns.
    unsafe {
        check(
            RoInitialize(RO_INIT_MULTITHREADED).map_or_else(|e| e.code(), |()| HRESULT(0)),
            "starting the runtime",
        )?;

        let library: Vec<u16> = "CoreMessaging.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(library.as_ptr());
        let export = GetProcAddress(module, c"CoreUICreateEx".as_ptr().cast())
            .ok_or("CoreMessaging has no CoreUICreateEx")?;
        let create_session: extern "system" fn(u32, *mut *mut c_void) -> HRESULT =
            std::mem::transmute(export);
        let mut session: *mut c_void = ptr::null_mut();
        check(create_session(1, &mut session), "creating the session")?;
        if session.is_null() {
            return Err("creating the session gave nothing".into());
        }

        let factory: IInspectable = RoGetActivationFactory(&HSTRING::from(MANAGER_CLASS))
            .map_err(|e| format!("finding the lifecycle manager: {e}"))?;
        let mut statics: *mut c_void = ptr::null_mut();
        check(
            (Interface::vtable(&factory).base.QueryInterface)(
                factory.as_raw(),
                &STATICS,
                &mut statics,
            ),
            "asking for the manager's statics",
        )?;
        let statics_vtable = *statics.cast::<*const usize>();
        let create: Method = std::mem::transmute(*statics_vtable.add(CREATE_SLOT));
        let mut made: *mut c_void = ptr::null_mut();
        check(create(statics, &mut made), "creating the lifecycle manager")?;
        tracing::info!("hosting Store app launches");

        let session_vtable = *session.cast::<*const usize>();
        let run: extern "system" fn(*mut c_void) -> HRESULT =
            std::mem::transmute(*session_vtable.add(RUN_SLOT));
        check(run(session), "running the session")
    }
}
