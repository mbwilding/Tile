//! The shell's second half of showing a Store app: its frame.
//!
//! A Store app's `Windows.UI.Core.CoreWindow` starts hidden by the shell (DWM's shell cloak) and
//! has no title bar. Explorer fixes both: it asks `ApplicationFrameHost` for a frame, puts the app
//! window inside it, and lifts the cloak. With no Explorer, [`start`] does the same for every app
//! window it finds, from a thread of its own.
//!
//! Lifting the cloak needs Immersive App Management access, which Windows only gives to the
//! process that owns the shell window, so [`start`] registers a hidden one. Like [`crate::uwp`],
//! this rests on internals read from the debug symbols of Windows 11 (build 26100): the frame
//! manager's and frame's vtable layouts, and the `AcquireIAMKey` and `EnableIAMAccess` calls. If an
//! update changes them the failure is logged and Store app windows stay hidden.

use std::{
    cell::OnceCell,
    collections::{HashMap, HashSet},
    ffi::c_void,
    mem::ManuallyDrop,
    ptr,
    rc::Rc,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use windows::{
    Win32::Foundation::E_FAIL,
    Win32::System::Com::{
        COINIT_MULTITHREADED, CoCreateFreeThreadedMarshaler, CoInitializeEx, IStream,
        Marshal::{IMarshal, IMarshal_Impl},
    },
    core::{
        GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, Result as ComResult, implement,
        interface,
    },
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, RECT},
    Graphics::Dwm::{DwmGetWindowAttribute, DwmSetWindowAttribute},
    Storage::Packaging::Appx::{GetApplicationUserModelId, GetPackageFullName},
    System::{
        Com::{CLSCTX_ALL, CLSCTX_LOCAL_SERVER, CoCreateInstance},
        LibraryLoader::{GetProcAddress, LoadLibraryW},
        Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, TerminateProcess,
        },
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, EnumChildWindows, EnumWindows, GetClassNameW,
        GetClientRect, GetParent, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId,
        IsWindow, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT,
        SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
        SetWindowTextW, ShowWindow, TranslateMessage, WM_CLOSE,
    },
};

/// `ApplicationFrameHost`'s class, which makes the frame manager.
const HOST_CLASS: GUID = GUID::from_u128(0xB9B0_5098_3E30_483F_87F7_027C_A78D_A287);
/// `IApplicationFrameManager`.
const FRAME_MANAGER: GUID = GUID::from_u128(0xA914_F499_4633_4B26_A93E_707E_B5BD_C0B6);
/// `IApplicationFrameManager::CreateFrame(IApplicationFrame**)`.
const CREATE_FRAME_SLOT: usize = 3;
/// `IApplicationFrameManager::RegisterForFrameEvents(handler, long*)`.
const REGISTER_EVENTS_SLOT: usize = 7;
/// The command a frame reports when its close button, `Alt+F4` or the system menu asks to close.
const CLOSE_COMMAND: GUID = GUID::from_u128(0x55BA_4179_C796_4426_81F0_4739_E926_5B57);
/// How long an app gets to close after being asked before it is ended.
const CLOSE_GRACE: Duration = Duration::from_secs(3);
/// `IApplicationFrameManager::DestroyFrame(IApplicationFrame*)`.
const DESTROY_FRAME_SLOT: usize = 5;
/// `IApplicationFrame::GetFrameWindow(HWND*)`.
const GET_FRAME_WINDOW_SLOT: usize = 3;
/// `IApplicationFrame::SetPresentedWindow(HWND)`.
const SET_PRESENTED_WINDOW_SLOT: usize = 6;
/// `IApplicationFrame::SetApplicationId(LPCWSTR, BOOL)`.
const SET_APPLICATION_ID_SLOT: usize = 9;

/// `PackageDebugSettings`, and its `IPackageDebugSettings`.
const PACKAGE_DEBUG_SETTINGS: GUID = GUID::from_u128(0xB1AE_C16F_2383_4852_B0E9_8F0B_1DC6_6B4D);
const PACKAGE_DEBUG_SETTINGS_IFACE: GUID =
    GUID::from_u128(0xF27C_3930_8029_4AD1_94E3_3DBA_4178_10C1);

const DWMWA_CLOAK: u32 = 13;
const DWMWA_CLOAKED: u32 = 14;
const DWM_CLOAKED_SHELL: u32 = 2;

/// Shell hosts whose hidden windows must be left alone.
const SHELL_HOSTS: [&str; 8] = [
    "applicationframehost.exe",
    "lockapp.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "widgets.exe",
    "widgetservice.exe",
];

/// How many scans try to frame one app window before giving up on it.
const MAX_TRIES: u8 = 20;
/// How often empty frames are looked for.
const SWEEP_EVERY: u32 = 33;
/// How many sweeps in a row a frame must be empty before it is destroyed.
const EMPTY_SWEEPS: u8 = 3;
/// `IApplicationFrame`.
const FRAME: GUID = GUID::from_u128(0x1437_15D9_A015_40EA_B695_D5CC_267E_36EE);
/// `IApplicationFrameManager::GetFrameArray(IObjectArray**)`.
const GET_FRAME_ARRAY_SLOT: usize = 6;
/// How often windows are checked for a new app to frame.
const SCAN_EVERY: u32 = 1;
/// How many ticks after framing an app is kept from staying frozen.
const RESUME_FOR: u32 = 200;
/// The pause between checks that keep frames and their apps the same size.
const TICK: Duration = Duration::from_millis(30);

#[link(name = "user32")]
unsafe extern "system" {
    /// Registers `window` as the shell's, if the desktop has none yet.
    fn SetShellWindow(window: HWND) -> i32;
}

type SetPackage = extern "system" fn(*mut c_void, *const u16) -> i32;
type Destroy = extern "system" fn(*mut c_void, *mut c_void) -> i32;
type Register = extern "system" fn(*mut c_void, *mut c_void, *mut i32) -> i32;
type Create = extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
type GetWindow = extern "system" fn(*mut c_void, *mut HWND) -> i32;
type SetWindow = extern "system" fn(*mut c_void, HWND) -> i32;
type SetId = extern "system" fn(*mut c_void, *const u16, i32) -> i32;
type Acquire = extern "system" fn(*mut usize) -> i32;
type Enable = extern "system" fn(*const usize, i32) -> i32;

/// A frame whose close was asked for.
struct CloseRequest(IUnknown);

// SAFETY: the frame is a proxy to an object in another process, which any thread of this
// multithreaded apartment may call.
unsafe impl Send for CloseRequest {}

/// Frames whose close was asked for, until the framer's thread deals with them.
static CLOSE_REQUESTS: Mutex<Vec<CloseRequest>> = Mutex::new(Vec::new());

/// `IApplicationFrameEventHandler`: what a frame tells the shell about itself.
#[interface("EA5D0DE4-770D-4DA0-A9F8-D7F9A140FF79")]
unsafe trait IApplicationFrameEventHandler: IUnknown {
    unsafe fn frame_changed(&self, frame: *mut c_void) -> HRESULT;
    unsafe fn frame_command(&self, frame: *mut c_void, command: *const GUID, value: i32)
    -> HRESULT;
    unsafe fn frame_ended(&self, frame: *mut c_void) -> HRESULT;
}

#[implement(IApplicationFrameEventHandler, IMarshal)]
struct Events {
    /// The free-threaded marshaler aggregated into this object, which is what lets the frame host
    /// call back from any of its threads (Explorer's own objects do the same).
    ftm: Rc<OnceCell<IMarshal>>,
}

impl Events {
    /// Makes the handler, with its marshaler already aggregated into it.
    fn make() -> ComResult<IApplicationFrameEventHandler> {
        let ftm = Rc::new(OnceCell::new());
        let handler: IApplicationFrameEventHandler = Events { ftm: ftm.clone() }.into();
        let outer: IUnknown = handler.cast()?;
        // SAFETY: plain COM call; the marshaler holds `outer` without counting it.
        let marshaler = unsafe { CoCreateFreeThreadedMarshaler(&outer)? }.cast::<IMarshal>()?;
        let _ = ftm.set(marshaler);
        Ok(handler)
    }

    fn ftm(&self) -> ComResult<&IMarshal> {
        self.ftm
            .get()
            .ok_or_else(|| windows::core::Error::from(E_FAIL))
    }
}

// Lets the frame host call back from any of its threads, by marshaling like the free-threaded
// marshaler does.
impl IMarshal_Impl for Events_Impl {
    fn GetUnmarshalClass(
        &self,
        riid: *const GUID,
        pv: *const c_void,
        context: u32,
        context_data: *const c_void,
        flags: u32,
    ) -> ComResult<GUID> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?
                .GetUnmarshalClass(riid, Some(pv), context, Some(context_data), flags)
        }
    }

    fn GetMarshalSizeMax(
        &self,
        riid: *const GUID,
        pv: *const c_void,
        context: u32,
        context_data: *const c_void,
        flags: u32,
    ) -> ComResult<u32> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?
                .GetMarshalSizeMax(riid, Some(pv), context, Some(context_data), flags)
        }
    }

    fn MarshalInterface(
        &self,
        stream: Option<&IStream>,
        riid: *const GUID,
        pv: *const c_void,
        context: u32,
        context_data: *const c_void,
        flags: u32,
    ) -> ComResult<()> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?
                .MarshalInterface(stream, riid, Some(pv), context, Some(context_data), flags)
        }
    }

    fn UnmarshalInterface(
        &self,
        stream: Option<&IStream>,
        riid: *const GUID,
        object: *mut *mut c_void,
    ) -> ComResult<()> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe { self.ftm()?.UnmarshalInterface(stream, riid, object) }
    }

    fn ReleaseMarshalData(&self, stream: Option<&IStream>) -> ComResult<()> {
        // SAFETY: forwards COM's own argument unchanged.
        unsafe { self.ftm()?.ReleaseMarshalData(stream) }
    }

    fn DisconnectObject(&self, reserved: u32) -> ComResult<()> {
        // SAFETY: forwards COM's own argument unchanged.
        unsafe { self.ftm()?.DisconnectObject(reserved) }
    }
}

impl IApplicationFrameEventHandler_Impl for Events_Impl {
    unsafe fn frame_changed(&self, _frame: *mut c_void) -> HRESULT {
        HRESULT(0)
    }

    unsafe fn frame_command(
        &self,
        frame: *mut c_void,
        command: *const GUID,
        _value: i32,
    ) -> HRESULT {
        // SAFETY: the frame host passes a live frame and a live GUID.
        unsafe {
            if command.as_ref() == Some(&CLOSE_COMMAND) && !frame.is_null() {
                let borrowed = ManuallyDrop::new(IUnknown::from_raw(frame));
                if let Ok(mut requests) = CLOSE_REQUESTS.lock() {
                    requests.push(CloseRequest((*borrowed).clone()));
                }
            }
        }
        HRESULT(0)
    }

    unsafe fn frame_ended(&self, _frame: *mut c_void) -> HRESULT {
        HRESULT(0)
    }
}

/// Starts framing Store app windows on a thread of its own, unless Explorer is the shell.
pub fn start() {
    let spawned = thread::Builder::new()
        .name("store frames".into())
        .spawn(|| match run() {
            Ok(()) => tracing::info!("the Store app framer stopped"),
            Err(e) => tracing::warn!("Store app windows will stay hidden: {e}"),
        });
    if let Err(e) = spawned {
        tracing::warn!("couldn't start the Store app framer: {e}");
    }
}

/// A Store app window and the frame it sits in.
struct Frame {
    /// When the app was asked to close.
    closing: Option<Instant>,
    /// The frame object, which goes away with its last reference.
    _com: IUnknown,
    manager: IUnknown,
    /// The app's package, which Windows may freeze while its window is hidden.
    package: String,
    /// Checks left that the app isn't frozen.
    resumes: u32,
    frame: HWND,
    app: HWND,
    size: (i32, i32),
    /// The app's title as last copied to the frame.
    title: Vec<u16>,
}

struct Framer {
    key: usize,
    enable: Enable,
    manager: Option<IUnknown>,
    /// Hears from every frame; kept alive for the life of the manager.
    events: Option<IApplicationFrameEventHandler>,
    /// The window management service, once found.
    statics: Option<IUnknown>,
    frames: Vec<Frame>,
    /// How many times each app window was tried, so one that keeps failing is given up on.
    tried: HashMap<usize, u8>,
    /// How many sweeps in a row each frame has held no app.
    empty: HashMap<usize, u8>,
}

fn run() -> Result<(), String> {
    // SAFETY: plain COM and window calls; the shell window is a live local owned by this thread,
    // which keeps pumping messages for it.
    let framer = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        take_shell()?
    };
    let mut framer = framer;
    if let Err(e) = crate::immersive::register() {
        tracing::warn!("couldn't register the shell stand-in: {e}");
    }
    let mut tick = 0u32;
    loop {
        pump();
        if tick.is_multiple_of(SCAN_EVERY) {
            framer.scan();
        }
        if tick.is_multiple_of(SWEEP_EVERY) {
            framer.sweep_empty();
        }
        framer.follow();
        tick = tick.wrapping_add(1);
        wait(TICK);
    }
}

/// Registers a hidden window as the shell window and takes the Immersive App Management key
/// that comes with it.
///
/// # Safety
///
/// Must run on the thread that pumps the shell window's messages.
unsafe fn take_shell() -> Result<Framer, String> {
    // SAFETY: the class is a built-in one; the arguments make a 1x1 window with no parent.
    let window = unsafe {
        CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            1,
            1,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err("couldn't make the shell window".into());
    }
    // SAFETY: `window` was just created on this thread.
    if unsafe { SetShellWindow(window) } == 0 {
        return Err("another process is the shell".into());
    }
    let library = wide("win32u.dll");
    // SAFETY: loading a system library by name.
    let module = unsafe { LoadLibraryW(library.as_ptr()) };
    // SAFETY: both exports have these signatures in Windows 11 build 26100.
    let (acquire, enable) = unsafe {
        let acquire = GetProcAddress(module, c"NtUserAcquireIAMKey".as_ptr().cast())
            .ok_or("win32u has no NtUserAcquireIAMKey")?;
        let enable = GetProcAddress(module, c"NtUserEnableIAMAccess".as_ptr().cast())
            .ok_or("win32u has no NtUserEnableIAMAccess")?;
        (
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Acquire>(acquire),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Enable>(enable),
        )
    };
    let mut key = 0usize;
    if acquire(&raw mut key) == 0 {
        return Err("Windows refused the Immersive App Management key".into());
    }
    tracing::info!("framing Store app windows");
    Ok(Framer {
        key,
        enable,
        manager: None,
        events: None,
        statics: None,
        frames: Vec::new(),
        tried: HashMap::new(),
        empty: HashMap::new(),
    })
}

impl Framer {
    /// Frames every new Store app window the shell is still hiding.
    fn scan(&mut self) {
        self.tried.retain(|&app, _| {
            // SAFETY: `IsWindow` accepts any handle value.
            unsafe { IsWindow(app as HWND) != 0 }
        });
        for app in top_level_windows() {
            let attempts = self.tried.get(&(app as usize)).copied().unwrap_or(0);
            if attempts >= MAX_TRIES || !is_hidden_app_window(app) {
                continue;
            }
            self.tried.insert(app as usize, attempts + 1);
            let Some((id, package)) = app_id(app) else {
                continue;
            };
            match self.frame(app, &id, package) {
                Ok(frame) => {
                    tracing::info!("framed {id}");
                    self.frames.push(frame);
                }
                Err(e) => tracing::warn!("couldn't frame {id}: {e}"),
            }
        }
    }

    /// Makes a frame for `app`, shows the app in it and lifts the cloak on both.
    fn frame(&mut self, app: HWND, id: &str, package: String) -> Result<Frame, String> {
        let first = self.try_frame(app, id, package.clone());
        if first.is_err() && self.manager.take().is_some() {
            // `ApplicationFrameHost` may have restarted since the manager was made.
            return self.try_frame(app, id, package);
        }
        first
    }

    /// The frame manager, made (and listened to) the first time it is needed.
    fn manager(&mut self) -> Result<IUnknown, String> {
        if let Some(manager) = &self.manager {
            return Ok(manager.clone());
        }
        let mut raw = ptr::null_mut();
        // SAFETY: asks `ApplicationFrameHost` for the frame manager directly; the GUIDs are live
        // constants and `raw` a live out pointer.
        let hr = unsafe {
            CoCreateInstance(
                (&HOST_CLASS as *const GUID).cast(),
                ptr::null_mut(),
                CLSCTX_LOCAL_SERVER,
                (&FRAME_MANAGER as *const GUID).cast(),
                &raw mut raw,
            )
        };
        check(hr, "starting ApplicationFrameHost")?;
        // SAFETY: `CoCreateInstance` returned an owned reference to the manager.
        let manager = unsafe { IUnknown::from_raw(raw) };
        self.manager = Some(manager.clone());
        self.listen(&manager);
        Ok(manager)
    }

    /// Destroys frames that hold no app and aren't being made: ones a previous helper left
    /// behind, or the frame host's own spares. Left alone they show as empty blue windows.
    fn sweep_empty(&mut self) {
        if self.manager.is_none() && !top_level_windows().into_iter().any(is_frame) {
            return;
        }
        let Ok(manager) = self.manager() else {
            return;
        };
        let mut array = ptr::null_mut();
        // SAFETY: `GetFrameArray` is slot 6 of the manager; `array` is a live out pointer.
        let hr = unsafe {
            let get: Create = std::mem::transmute(slot(manager.as_raw(), GET_FRAME_ARRAY_SLOT));
            get(manager.as_raw(), &raw mut array)
        };
        if hr < 0 || array.is_null() {
            return;
        }
        // SAFETY: the array is an owned `IObjectArray`, whose `GetCount` is slot 3 and `GetAt`
        // slot 4; every pointer passed is a live local.
        let found = unsafe {
            let array = IUnknown::from_raw(array);
            let count: extern "system" fn(*mut c_void, *mut u32) -> i32 =
                std::mem::transmute(slot(array.as_raw(), 3));
            let at: extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void) -> i32 =
                std::mem::transmute(slot(array.as_raw(), 4));
            let mut n = 0u32;
            count(array.as_raw(), &raw mut n);
            let mut found = Vec::new();
            for i in 0..n {
                let mut raw = ptr::null_mut();
                if at(array.as_raw(), i, &FRAME, &raw mut raw) >= 0 && !raw.is_null() {
                    found.push(IUnknown::from_raw(raw));
                }
            }
            found
        };
        let mut seen = HashSet::new();
        for frame in found {
            let mut window: HWND = ptr::null_mut();
            // SAFETY: `GetFrameWindow` is slot 3 of `IApplicationFrame`; both pointers are live.
            let ok = unsafe {
                let get: GetWindow =
                    std::mem::transmute(slot(frame.as_raw(), GET_FRAME_WINDOW_SLOT));
                get(frame.as_raw(), &raw mut window) >= 0 && !window.is_null()
            };
            if !ok || self.frames.iter().any(|f| f.frame == window) || holds_app(window) {
                continue;
            }
            seen.insert(window as usize);
            let sweeps = self.empty.entry(window as usize).or_insert(0);
            *sweeps += 1;
            if *sweeps >= EMPTY_SWEEPS {
                // SAFETY: `DestroyFrame` is slot 5 of the manager; both objects are alive.
                unsafe {
                    let destroy: Destroy =
                        std::mem::transmute(slot(manager.as_raw(), DESTROY_FRAME_SLOT));
                    destroy(manager.as_raw(), frame.as_raw());
                }
            }
        }
        self.empty.retain(|window, _| seen.contains(window));
    }

    /// Asks the frame host to report each frame's commands, such as close, to this process.
    fn listen(&mut self, manager: &IUnknown) {
        let handler = match Events::make() {
            Ok(handler) => handler,
            Err(e) => {
                tracing::warn!("couldn't make the frame event handler: {e}");
                return;
            }
        };
        let mut cookie = 0;
        // SAFETY: slot 7 is `RegisterForFrameEvents` in Windows 11 build 26100; both pointers are
        // live and the handler is kept until the manager is replaced.
        let hr = unsafe {
            let register: Register =
                std::mem::transmute(slot(manager.as_raw(), REGISTER_EVENTS_SLOT));
            register(manager.as_raw(), handler.as_raw(), &raw mut cookie)
        };
        match check(hr, "listening for frame commands") {
            Ok(()) => tracing::info!("listening for frame commands, cookie {cookie}"),
            Err(e) => tracing::warn!("{e}"),
        }
        self.events = Some(handler);
    }

    fn try_frame(&mut self, app: HWND, id: &str, package: String) -> Result<Frame, String> {
        let manager = self.manager()?;
        let id: Vec<u16> = id.encode_utf16().chain([0]).collect();
        let mut raw = ptr::null_mut();
        let mut window: HWND = ptr::null_mut();
        // SAFETY: the vtable slots and argument lists are those of Windows 11 build 26100;
        // `manager` is alive and `frame` is a live out pointer.
        let frame = unsafe {
            let create: Create = std::mem::transmute(slot(manager.as_raw(), CREATE_FRAME_SLOT));
            check(create(manager.as_raw(), &raw mut raw), "creating the frame")?;
            IUnknown::from_raw(raw)
        };
        let mut orphan = Orphan {
            manager: &manager,
            frame: &frame,
            armed: true,
        };
        // SAFETY: as above; `raw` is the frame just made.
        unsafe {
            let set_id: SetId = std::mem::transmute(slot(raw, SET_APPLICATION_ID_SLOT));
            check(set_id(raw, id.as_ptr(), 1), "naming the frame")?;
            let present: SetWindow = std::mem::transmute(slot(raw, SET_PRESENTED_WINDOW_SLOT));
            check(present(raw, app), "putting the app in the frame")?;
            let get: GetWindow = std::mem::transmute(slot(raw, GET_FRAME_WINDOW_SLOT));
            check(get(raw, &raw mut window), "finding the frame's window")?;
        }
        if window.is_null() {
            return Err("the frame has no window".into());
        }
        // Everything is in place before the cloak lifts, which is when Tile hears of the window:
        // its title, and a size and place for it that Tile then replaces with a tile.
        let title = set_title(window, app);
        let (width, height) = initial_size();
        // SAFETY: `window` is the frame's, which this process just made.
        unsafe {
            SetWindowPos(
                window,
                ptr::null_mut(),
                (screen(SM_CXSCREEN) - width) / 2,
                (screen(SM_CYSCREEN) - height) / 2,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            ShowWindow(window, SW_SHOW);
        }
        (self.enable)(&raw const self.key, 1);
        for hwnd in [window, app] {
            let clear = 0i32;
            // SAFETY: `clear` outlives the call and is the 4 bytes DWMWA_CLOAK takes.
            unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as _, (&raw const clear).cast(), 4) };
        }
        (self.enable)(&raw const self.key, 0);
        orphan.armed = false;
        drop(orphan);
        Ok(Frame {
            closing: None,
            frame: window,
            app,
            _com: frame,
            manager: manager.clone(),
            package,
            resumes: RESUME_FOR,
            size: (0, 0),
            title,
        })
    }

    /// Keeps each app the size of its frame, and closes whichever of the two outlives the other.
    fn follow(&mut self) {
        let requests = CLOSE_REQUESTS
            .lock()
            .map(|mut requests| std::mem::take(&mut *requests))
            .unwrap_or_default();
        for request in requests {
            let mut window: HWND = ptr::null_mut();
            // SAFETY: `GetFrameWindow` is slot 3 of `IApplicationFrame`; the request holds a live
            // reference to the frame and `window` is a live out pointer.
            let found = unsafe {
                let raw = request.0.as_raw();
                let get: GetWindow = std::mem::transmute(slot(raw, GET_FRAME_WINDOW_SLOT));
                get(raw, &raw mut window) >= 0
            };
            if let Some(f) = self.frames.iter_mut().find(|f| found && f.frame == window) {
                f.closing.get_or_insert_with(Instant::now);
                // SAFETY: a plain hide of the frame and message post to the app's window.
                unsafe {
                    // Gone at once; the app gets its grace period to exit out of sight.
                    ShowWindow(f.frame, SW_HIDE);
                    PostMessageW(f.app, WM_CLOSE, 0, 0);
                }
            }
        }
        if self.statics.is_none() {
            self.statics = window_statics();
        }
        let statics = self.statics.clone();
        self.frames.retain_mut(|f| {
            // SAFETY: `IsWindow` accepts any handle value.
            let (frame, app) = unsafe { (IsWindow(f.frame) != 0, IsWindow(f.app) != 0) };
            if !frame || !app {
                if frame {
                    // SAFETY: `IApplicationFrameManager::DestroyFrame(IApplicationFrame*)` is slot
                    // 5; both objects are alive and belong together.
                    unsafe {
                        let destroy: Destroy =
                            std::mem::transmute(slot(f.manager.as_raw(), DESTROY_FRAME_SLOT));
                        destroy(f.manager.as_raw(), f._com.as_raw());
                    }
                } else if app {
                    // SAFETY: a plain message post to a window that is still alive.
                    unsafe { PostMessageW(f.app, WM_CLOSE, 0, 0) };
                }
                return false;
            }
            if f.resumes > 0 {
                f.resumes -= 1;
                if f.resumes % 10 == 0 {
                    resume(&f.package);
                }
            }
            if f.closing.is_some_and(|asked| asked.elapsed() > CLOSE_GRACE) {
                end_process(f.app);
            }
            copy_title(f);
            let mut client = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // SAFETY: `client` is a live out pointer and `f.frame` a live window.
            unsafe { GetClientRect(f.frame, &raw mut client) };
            let size = (client.right, client.bottom);
            if size != f.size && size.0 > 0 && size.1 > 0 {
                f.size = size;
                if let Some(statics) = &statics {
                    set_bounds(statics, f.app, size.0, size.1);
                }
            }
            true
        });
    }
}

/// Thaws the package, which Windows freezes when an app launches with no visible window.
fn resume(package: &str) {
    let name = wide(package);
    let mut raw = ptr::null_mut();
    // SAFETY: `IPackageDebugSettings::Resume` is slot 6 of its vtable; the GUIDs are live
    // constants, `raw` is a live out pointer and `name` is NUL-terminated.
    unsafe {
        let hr = CoCreateInstance(
            (&PACKAGE_DEBUG_SETTINGS as *const GUID).cast(),
            ptr::null_mut(),
            CLSCTX_ALL,
            (&PACKAGE_DEBUG_SETTINGS_IFACE as *const GUID).cast(),
            &raw mut raw,
        );
        if hr < 0 || raw.is_null() {
            return;
        }
        let settings = IUnknown::from_raw(raw);
        let thaw: SetPackage = std::mem::transmute(slot(raw, 6));
        thaw(raw, name.as_ptr());
        drop(settings);
    }
}

/// Gives `frame` the title of the app window `app`, which is what Windows (and Tile) use to tell
/// windows apart and to decide which to manage. Returns the title, empty if the app has none yet.
fn set_title(frame: HWND, app: HWND) -> Vec<u16> {
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` has room for the length given and `app` is a live window.
    let length = unsafe { GetWindowTextW(app, buffer.as_mut_ptr(), buffer.len() as i32) };
    let mut title = buffer[..length.max(0) as usize].to_vec();
    if !title.is_empty() {
        title.push(0);
        // SAFETY: `title` is NUL-terminated and `frame` is a live window.
        unsafe { SetWindowTextW(frame, title.as_ptr()) };
        title.pop();
    }
    title
}

/// Copies the app's title to its frame again when it has changed.
fn copy_title(f: &mut Frame) {
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` has room for the length given and `f.app` is a live window.
    let length = unsafe { GetWindowTextW(f.app, buffer.as_mut_ptr(), buffer.len() as i32) };
    if length > 0 && buffer[..length as usize] != f.title[..] {
        f.title = set_title(f.frame, f.app);
    }
}

/// A frame that isn't in use yet, destroyed again if making it fails part way.
struct Orphan<'a> {
    manager: &'a IUnknown,
    frame: &'a IUnknown,
    armed: bool,
}

impl Drop for Orphan<'_> {
    fn drop(&mut self) {
        if self.armed {
            // SAFETY: `DestroyFrame` is slot 5 of the manager; both objects are alive.
            unsafe {
                let destroy: Destroy =
                    std::mem::transmute(slot(self.manager.as_raw(), DESTROY_FRAME_SLOT));
                destroy(self.manager.as_raw(), self.frame.as_raw());
            }
        }
    }
}

/// Ends the process that owns `window`, for an app that didn't close when asked.
fn end_process(window: HWND) {
    let mut pid = 0;
    // SAFETY: `pid` is a live out pointer; the handle is closed again below.
    unsafe {
        GetWindowThreadProcessId(window, &raw mut pid);
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !process.is_null() {
            TerminateProcess(process, 0);
            CloseHandle(process);
        }
    }
}

fn check(hr: i32, what: &str) -> Result<(), String> {
    if hr < 0 {
        Err(format!("{what}: 0x{:08x}", hr as u32))
    } else {
        Ok(())
    }
}

/// The function at `index` of the vtable of the COM object `object`.
///
/// # Safety
///
/// `object` must be a live COM object with at least `index + 1` methods.
unsafe fn slot(object: *mut c_void, index: usize) -> *const c_void {
    // SAFETY: a COM object starts with a pointer to its vtable.
    unsafe { *(*object.cast::<*const *const c_void>()).add(index) }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn screen(metric: i32) -> i32 {
    // SAFETY: `GetSystemMetrics` takes no pointers.
    unsafe { GetSystemMetrics(metric) }
}

/// What the window management service takes: a window's id (its handle) and a rectangle.
type SetRect = extern "system" fn(*mut c_void, u64, *const [f32; 4]) -> i32;

/// `Windows.Internal.ApplicationModel.WindowManagement.UniversalAppModelInProc`'s statics
/// (`IUniversalAppModelInProcStatics`).
const WINDOW_STATICS: GUID = GUID::from_u128(0xB880_CE78_2E50_5A73_BFAA_A3F9_A8B2_F042);
/// `SetBounds(WindowId, Rect)`.
const SET_BOUNDS_SLOT: usize = 13;
/// `SetVisibleBounds(WindowId, Rect)`.
const SET_VISIBLE_BOUNDS_SLOT: usize = 27;

/// The window management service's statics, which tell an app its window's size.
///
/// Without Explorer nothing else does, and a Store app keeps laying itself out for the size its
/// window was created with, which is that of the whole monitor. Only a process that hosts the
/// shell side (this one) is answered.
fn window_statics() -> Option<IUnknown> {
    use windows::{
        Win32::System::WinRT::RoGetActivationFactory,
        core::{HSTRING, IInspectable},
    };
    // SAFETY: plain WinRT activation and a query for a statics interface.
    unsafe {
        let factory: IInspectable = RoGetActivationFactory(&HSTRING::from(
            "Windows.Internal.ApplicationModel.WindowManagement.UniversalAppModelInProc",
        ))
        .ok()?;
        let mut raw = ptr::null_mut();
        factory.query(&WINDOW_STATICS, &raw mut raw).ok().ok()?;
        Some(IUnknown::from_raw(raw))
    }
}

/// Tells `app` that its window is `width` by `height` (and sizes the window to match).
fn set_bounds(statics: &IUnknown, app: HWND, width: i32, height: i32) {
    let rect = [0.0, 0.0, width as f32, height as f32];
    for slot_index in [SET_BOUNDS_SLOT, SET_VISIBLE_BOUNDS_SLOT] {
        // SAFETY: both slots take `(WindowId, Rect)` in Windows 11 build 26100; `statics` is alive
        // and `rect` is a live local.
        unsafe {
            let call: SetRect = std::mem::transmute(slot(statics.as_raw(), slot_index));
            call(statics.as_raw(), app as u64, &rect);
        }
    }
}

/// A frame size that suits the screen: 1200x800, or three quarters of a smaller one.
fn initial_size() -> (i32, i32) {
    (
        1200.min(screen(SM_CXSCREEN) * 3 / 4),
        800.min(screen(SM_CYSCREEN) * 3 / 4),
    )
}

/// Dispatches the thread's pending window messages.
fn pump() {
    // SAFETY: `message` is a zeroed `MSG`, which `PeekMessageW` fills in.
    unsafe {
        let mut message = std::mem::zeroed();
        while PeekMessageW(&raw mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

/// Sleeps for `time`, waking early for a message.
fn wait(time: Duration) {
    // SAFETY: waits on no handles, only for input.
    unsafe {
        MsgWaitForMultipleObjects(0, ptr::null(), 0, time.as_millis() as u32, QS_ALLINPUT);
    }
}

/// Whether `window` is a Store app frame.
fn is_frame(window: HWND) -> bool {
    let mut class = [0u16; 64];
    // SAFETY: `class` has room for the length given.
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    String::from_utf16_lossy(&class[..length.max(0) as usize]) == "ApplicationFrameWindow"
}

/// Whether `frame` has a Store app's window inside it.
fn holds_app(frame: HWND) -> bool {
    unsafe extern "system" fn look(window: HWND, found: LPARAM) -> i32 {
        let mut class = [0u16; 64];
        // SAFETY: `class` has room for the length given; `found` is the flag passed below.
        unsafe {
            let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
            if String::from_utf16_lossy(&class[..length.max(0) as usize])
                == "Windows.UI.Core.CoreWindow"
            {
                *(found as *mut bool) = true;
                return 0;
            }
        }
        1
    }
    let mut found = false;
    // SAFETY: `look` only touches the flag behind the pointer.
    unsafe { EnumChildWindows(frame, Some(look), (&raw mut found) as LPARAM) };
    found
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> i32 {
        // SAFETY: `list` is the `Vec` passed below, alive for the whole enumeration.
        unsafe { (*(list as *mut Vec<HWND>)).push(window) };
        1
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `collect` only touches the `Vec` behind the pointer.
    unsafe { EnumWindows(Some(collect), (&raw mut windows) as LPARAM) };
    windows
}

/// Whether `window` is a Store app's window that the shell is still hiding, with nothing around
/// it yet.
fn is_hidden_app_window(window: HWND) -> bool {
    let mut class = [0u16; 64];
    // SAFETY: `class` has room for the length given.
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    if String::from_utf16_lossy(&class[..length.max(0) as usize]) != "Windows.UI.Core.CoreWindow" {
        return false;
    }
    let mut cloak = 0u32;
    // SAFETY: `cloak` is the 4 bytes DWMWA_CLOAKED fills in.
    let read =
        unsafe { DwmGetWindowAttribute(window, DWMWA_CLOAKED as _, (&raw mut cloak).cast(), 4) };
    // SAFETY: `GetParent` accepts any handle value.
    read >= 0 && cloak & DWM_CLOAKED_SHELL != 0 && unsafe { GetParent(window) }.is_null()
}

/// The app user model id and package full name of the Store app that owns `window`, unless it's
/// a shell host.
fn app_id(window: HWND) -> Option<(String, String)> {
    let mut pid = 0;
    // SAFETY: `pid` is a live out pointer.
    unsafe { GetWindowThreadProcessId(window, &raw mut pid) };
    // SAFETY: a query-only handle that is closed again below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let mut path = [0u16; 520];
    let mut path_length = path.len() as u32;
    let mut id = [0u16; 130];
    let mut id_length = id.len() as u32;
    let mut package = [0u16; 280];
    let mut package_length = package.len() as u32;
    // SAFETY: the buffers have room for the lengths given and `process` is open.
    let (named, packaged) = unsafe {
        let named =
            QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &raw mut path_length) != 0;
        let packaged = GetApplicationUserModelId(process, &raw mut id_length, id.as_mut_ptr()) == 0
            && GetPackageFullName(process, &raw mut package_length, package.as_mut_ptr()) == 0;
        CloseHandle(process);
        (named, packaged)
    };
    if !named || !packaged {
        return None;
    }
    let path = String::from_utf16_lossy(&path[..path_length as usize]);
    let exe = path.rsplit('\\').next().unwrap_or_default().to_lowercase();
    if SHELL_HOSTS.contains(&exe.as_str()) {
        return None;
    }
    Some((
        String::from_utf16_lossy(&id[..id_length.saturating_sub(1) as usize]),
        String::from_utf16_lossy(&package[..package_length.saturating_sub(1) as usize]),
    ))
}
