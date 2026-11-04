//! What a host's native pays before it works: the bytes of its arguments ([`register_charged`]), a loop's rounds ([`pay`]) and
//! the file host's hidden work ([`charged_host`]). A run that cannot pay ends with the step limit before the work is done.

use ts_vm::host::FileHost;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

/// Registers a native that is charged the bytes of its arguments first: a loop over them is the native's own
/// work (`atoi`, `atob`, `sscanf` read them whole; a setter copies them), and a run that cannot pay ends
/// with the step limit before the native does anything.
pub fn register_charged(
    vm: &mut Vm,
    name: &str,
    argc: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
) {
    register_charged_range(vm, name, argc, argc, usage, f);
}

/// [`register_charged`] for a native whose console record takes `min..=max` arguments.
pub fn register_charged_range(
    vm: &mut Vm,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
) {
    vm.register(name, min, max, usage, move |vm, c| {
        let bytes = c
            .argv
            .iter()
            .skip(1)
            .map(Vec::len)
            .fold(0usize, usize::saturating_add);
        if bytes > 0 && !vm.charge_work(u64::try_from(bytes).unwrap_or(u64::MAX)) {
            return Native::Fail(VmError::StepLimit);
        }
        f(vm, c)
    });
}

/// Charges `units` of work to the run in progress; `Err(StepLimit)` once the budget cannot pay (the caller
/// then does none of the work: a host's loop calls this before each round, as the VM's natives do).
pub fn pay(vm: &Vm, units: u64) -> Result<(), VmError> {
    if vm.charge_work(units) {
        Ok(())
    } else {
        Err(VmError::StepLimit)
    }
}

/// One call into the file host, charged for the work the host did that no returned byte shows
/// (`FileHost::work_done`: a zip entry inflated for a read that then failed), as the VM's own file natives do.
/// The bytes a read returns are the caller's to charge (before it asks, from a `stat`).
pub fn charged_host<T>(vm: &Vm, call: impl FnOnce(&dyn FileHost) -> T) -> Result<T, VmError> {
    let host = vm.file_host();
    let mark = host.work_done();
    let out = call(host);
    pay(vm, host.work_done().saturating_sub(mark))?;
    Ok(out)
}
