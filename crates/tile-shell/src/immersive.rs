//! A stand-in for the shell's `ImmersiveShell` COM class.
//!
//! Store apps ask for this class to reach the shell's services, such as the application view
//! collection. Only Explorer registers it, so without Explorer the apps log "class not
//! registered" and some of their pages stay empty. [`register`] registers a service provider
//! that answers every request with "no such service", and logs what was asked for, which is
//! what a Store app does when the shell is unavailable.

use windows::{
    Win32::{
        Foundation::{CLASS_E_NOAGGREGATION, E_NOINTERFACE},
        System::Com::{
            CLSCTX_LOCAL_SERVER, CoRegisterClassObject, IClassFactory, IClassFactory_Impl,
            IServiceProvider, IServiceProvider_Impl, REGCLS_MULTIPLEUSE,
        },
    },
    core::{BOOL, GUID, IUnknown, Interface, Ref, Result, implement},
};

/// `CLSID_ImmersiveShell`.
const IMMERSIVE_SHELL: GUID = GUID::from_u128(0xC2F0_3A33_21F5_47FA_B4BB_1563_62A2_F239);

#[implement(IServiceProvider)]
struct Services;

impl IServiceProvider_Impl for Services_Impl {
    fn QueryService(
        &self,
        service: *const GUID,
        interface: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        // SAFETY: COM passes live pointers for the GUIDs and the out pointer.
        unsafe {
            tracing::info!(
                "the shell was asked for service {:?} as {:?}",
                service.as_ref(),
                interface.as_ref()
            );
            if !object.is_null() {
                *object = std::ptr::null_mut();
            }
        }
        Err(E_NOINTERFACE.into())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        interface: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let services: IServiceProvider = Services.into();
        // SAFETY: COM passes live pointers for the interface id and the out pointer.
        unsafe { services.query(interface, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// Registers the class for other processes to create. Call from a thread whose COM apartment
/// lives for the rest of the process.
pub fn register() -> Result<()> {
    let factory: IClassFactory = Factory.into();
    // SAFETY: registers a live factory; the registration lasts until the process ends.
    unsafe {
        CoRegisterClassObject(
            &IMMERSIVE_SHELL,
            &factory,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE,
        )?;
    }
    Ok(())
}
