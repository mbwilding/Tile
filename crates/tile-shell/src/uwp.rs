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

use std::{ffi::c_void, mem::ManuallyDrop, ptr, thread};

use windows::{
    Win32::{
        System::{
            Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            WinRT::{RO_INIT_MULTITHREADED, RoGetActivationFactory, RoInitialize},
        },
        UI::Shell::{ACTIVATEOPTIONS, ApplicationActivationManager, IApplicationActivationManager},
    },
    core::{
        HRESULT, HSTRING, IInspectable, IUnknown, IUnknown_Vtbl, Interface, OutRef, PCWSTR, Result,
        interface,
    },
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::win::{context, file_name, pid_of, process_path, wide, windows_of_class};

/// The shell component that completes Store app launches for `sihost`.
const MANAGER_CLASS: &str =
    "Windows.Internal.Shell.UwpWindowLifecycleManagement.UwpWindowLifecycleManager";

/// `IUwpWindowLifecycleManagerStatics`. It derives from `IInspectable`, whose three methods come
/// first.
#[interface("6C691047-8DB8-5D6E-A382-AE35ACDA1DD5")]
unsafe trait IUwpWindowLifecycleManagerStatics: IUnknown {
    fn _get_iids(&self);
    fn _get_runtime_class_name(&self);
    fn _get_trust_level(&self);
    fn _get_current(&self);
    fn create(&self, manager: OutRef<IUnknown>) -> Result<()>;
}

/// The session `CoreUICreateEx` makes. Its interface id isn't known, and isn't needed: the
/// session is only called, never asked for.
#[interface("00000000-0000-0000-0000-000000000000")]
unsafe trait ICoreUiSession: IUnknown {
    fn _3(&self);
    fn _4(&self);
    fn _5(&self);
    fn _6(&self);
    fn _7(&self);
    fn _8(&self);
    fn _9(&self);
    fn _10(&self);
    fn _11(&self);
    fn _12(&self);
    fn _13(&self);
    fn _14(&self);
    fn _15(&self);
    fn _16(&self);
    fn _17(&self);
    fn _18(&self);
    fn _19(&self);
    fn _20(&self);
    fn _21(&self);
    fn _22(&self);
    fn _23(&self);
    fn _24(&self);
    fn _25(&self);
    fn _26(&self);
    fn _27(&self);
    fn _28(&self);
    fn run(&self) -> Result<()>;
}

/// `CoreUICreateEx(kind, session*)`, which CoreMessaging exports without a header.
type CreateSession = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;

/// Whether Explorer owns a taskbar window, which makes it the shell.
fn explorer_is_shell() -> bool {
    windows_of_class("Shell_TrayWnd").into_iter().any(|window| {
        process_path(pid_of(window))
            .is_some_and(|path| file_name(&path).eq_ignore_ascii_case("explorer.exe"))
    })
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
pub fn activate(aumid: &str) -> std::result::Result<u32, String> {
    let id = wide(aumid);
    // SAFETY: plain COM calls; `id` is NUL-terminated and outlives the call.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let manager: IApplicationActivationManager = context(
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL),
            "creating the activation manager",
        )?;
        context(
            manager.ActivateApplication(PCWSTR(id.as_ptr()), PCWSTR::null(), ACTIVATEOPTIONS(0)),
            &format!("starting {aumid}"),
        )
    }
}

/// Makes a CoreMessaging session, creates the lifecycle manager from its thread, then runs the
/// session, which returns only when the process ends.
fn host() -> std::result::Result<(), String> {
    // SAFETY: takes no pointers.
    let started = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    context(started, "starting the runtime")?;

    let library = wide("CoreMessaging.dll");
    // SAFETY: loading a system library by name and looking up an export that has this signature
    // in Windows 11 build 26100. It is looked up rather than linked, so an update that drops it
    // stops only this.
    let create_session = unsafe {
        let module = LoadLibraryW(library.as_ptr());
        let export = GetProcAddress(module, c"CoreUICreateEx".as_ptr().cast())
            .ok_or("CoreMessaging has no CoreUICreateEx")?;
        std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateSession>(export)
    };
    let mut raw = ptr::null_mut();
    // SAFETY: `raw` is a live out pointer.
    let made = unsafe { create_session(1, &raw mut raw) };
    context(made.ok(), "creating the session")?;
    if raw.is_null() {
        return Err("creating the session gave nothing".into());
    }
    // Never released: it is run until the process ends, and how it counts references isn't
    // known.
    // SAFETY: `raw` is the session just made, whose vtable has `Run` in the slot declared.
    let session = ManuallyDrop::new(unsafe { ICoreUiSession::from_raw(raw) });

    // SAFETY: plain WinRT activation.
    let factory = unsafe { RoGetActivationFactory::<IInspectable>(&HSTRING::from(MANAGER_CLASS)) };
    let factory = context(factory, "finding the lifecycle manager")?;
    let statics: IUwpWindowLifecycleManagerStatics =
        context(factory.cast(), "asking for the manager's statics")?;
    let mut manager = None;
    // SAFETY: `manager` is a live out slot; this thread belongs to the session.
    let created = unsafe { statics.create(&mut manager) };
    context(created, "creating the lifecycle manager")?;
    tracing::info!("hosting Store app launches");
    // The manager is kept until the session ends.
    let _manager = manager;
    // SAFETY: a call on the live session.
    context(unsafe { session.run() }, "running the session")
}
