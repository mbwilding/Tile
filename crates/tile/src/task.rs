//! The scheduled tasks `tile install` registers and removes, through the Task Scheduler's COM
//! interface.

use std::path::Path;

use winsafe::{self as w, co, prelude::*};

/// The scheduled task that starts Tile elevated at sign-in.
pub const TILE: &str = "Tile";
/// The scheduled task of Key, the standalone remapper Tile absorbed.
pub const KEY: &str = "Key";

/// Joins COM for this thread and connects to the Task Scheduler.
fn connect() -> anyhow::Result<(w::guard::CoUninitializeGuard, w::ITaskService)> {
    let com = w::CoInitializeEx(co::COINIT::APARTMENTTHREADED)?;
    let service = w::CoCreateInstance::<w::ITaskService>(
        &co::CLSID::TaskScheduler,
        None::<&w::IUnknown>,
        co::CLSCTX::INPROC_SERVER,
    )?;
    service.Connect(None, None, None, None)?;
    Ok((com, service))
}

/// Whether the task `name` is registered.
pub fn exists(name: &str) -> bool {
    let Ok((_com, service)) = connect() else {
        return false;
    };
    service
        .GetFolder("\\")
        .is_ok_and(|folder| folder.GetTask(name).is_ok())
}

/// Registers [`TILE`], which starts `exe` elevated when `user` signs in.
pub fn register(exe: &Path, user: &str) -> anyhow::Result<()> {
    let (_com, service) = connect()?;
    let folder = service.GetFolder("\\")?;
    let task = service.NewTask()?;
    task.get_Principal()?
        .put_RunLevel(co::TASK_RUNLEVEL::HIGHEST)?;
    // The defaults would stop Tile after 3 days or on battery, and run it at low priority.
    let settings = task.get_Settings()?;
    settings.put_DisallowStartIfOnBatteries(false)?;
    settings.put_StopIfGoingOnBatteries(false)?;
    settings.put_ExecutionTimeLimit("PT0S")?;
    settings.put_MultipleInstances(co::TASK_INSTANCES::IGNORE_NEW)?;
    settings.put_Priority(4)?;
    let trigger = task
        .get_Triggers()?
        .Create(co::TASK_TRIGGER_TYPE2::LOGON)?
        .QueryInterface::<w::ILogonTrigger>()?;
    trigger.put_UserId(user)?;
    let action = task
        .get_Actions()?
        .Create(co::TASK_ACTION_TYPE::EXEC)?
        .QueryInterface::<w::IExecAction>()?;
    action.put_Path(&tile_platform::conhost().to_string_lossy())?;
    action.put_Arguments(&format!("--headless \"{}\"", exe.display()))?;
    folder.RegisterTaskDefinition(
        Some(TILE),
        &task,
        co::TASK_CREATION::CREATE_OR_UPDATE,
        Some(user),
        None,
        co::TASK_LOGON::INTERACTIVE_TOKEN,
        None,
    )?;
    Ok(())
}

/// Removes the task `name`, if it is registered.
pub fn delete(name: &str) -> anyhow::Result<()> {
    let (_com, service) = connect()?;
    let folder = service.GetFolder("\\")?;
    if folder.GetTask(name).is_ok() {
        folder.DeleteTask(name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_that_was_never_registered_does_not_exist() {
        assert!(!exists("Tile test task that does not exist"));
        delete("Tile test task that does not exist").unwrap();
    }
}
