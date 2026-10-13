//! Sangfor VDI client plugin entry points.
//!
//! The Sangfor VDI client does not use the standard `FreeRDP` names to load its
//! RDP virtual channel plugins: it resolves `svc_plugin_init` or
//! `dvc_plugin_init` instead. Both strings are referenced by `libngs2.dll`,
//! the `FreeRDP` fork embedded in the client, and by `RVLSession.exe` which
//! loads the plugin DLLs listed in its own hardcoded table.
//!
//! # Calling convention
//!
//! The signature below has been confirmed by reverse engineering the channel
//! dispatcher of `libngs2.dll` (function at `0x10005b90` of the 5.5.3 build):
//! it loads the entry point from a global, pushes four arguments, calls it and
//! cleans the stack itself with `add esp, 0x10`, which means `cdecl`. The
//! 32-bit return value is then tested against zero.
//!
//! The two first arguments are checked against null by the plugins shipped with
//! the client (`librdpclip.dll`, `librdpgfx.dll`, `librvlclip.dll`, ...), and
//! the second one is a structure whose `+4` field points to a descriptor whose
//! first `DWORD` is the channel type:
//!
//! | value | plugin                | kind                    |
//! |-------|-----------------------|-------------------------|
//! | `2`   | `librdpclip/dr/snd`   | static virtual channel  |
//! | `5`   | `librvlclip`          | dynamic virtual channel |
//! | `9`   | `librdpgfx`           | dynamic virtual channel |
//!
//! The third and fourth arguments are forwarded by the dispatcher but are not
//! used by the plugins of the client, so their meaning is still unknown.
//!
//! # Integrity
//!
//! Be aware that every Sangfor binary which loads another module imports
//! `WinVerifyTrustEx`, `CryptQueryObject`, `CryptCreateHash` and
//! `CreateToolhelp32Snapshot`, and contains the strings *"Loading failed.
//! Please check if the signature of file (%s) is valid."*. The client does
//! verify the Authenticode signature of the files it loads, which includes the
//! patched binaries (see the `sangfor` mode of `soxyreg`).

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// Number of bytes of the entry point arguments which are dumped.
const DUMP_LEN: usize = 64;

/// Name of the log file, created in the temporary directory of the current
/// user.
const LOG_NAME: &str = "soxy-sangfor-probe.log";

fn log_path() -> PathBuf {
    std::env::temp_dir().join(LOG_NAME)
}

fn append(line: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Cheap sanity check, used to report how many values of the dumped structure
/// look like user mode function pointers.
#[allow(clippy::manual_range_contains, reason = "not const stable yet")]
const fn looks_like_fn_ptr(value: u32) -> bool {
    value >= 0x1_0000 && value < 0x8000_0000
}

/// Dumps the arguments of a Sangfor plugin entry point.
///
/// The channel descriptor is read as well: its layout has been confirmed by
/// three independent plugins of the client, so it is safe to dereference. The
/// raw dump is written to the log file first, hence it is available even if
/// this last read were to fault.
///
/// # Safety
///
/// `context`, `channels` and `descriptor` must either be null or point to at
/// least [`DUMP_LEN`] readable bytes, which is what a plugin entry point is
/// expected to receive.
unsafe fn probe(entry: &str, context: *mut c_void, channels: *mut c_void) -> i32 {
    append(&format!("{entry} called with {context:p} {channels:p}"));

    for (name, argument) in [("context", context), ("channels", channels)] {
        if argument.is_null() {
            append(&format!("  {name} is NULL"));
            continue;
        }

        let bytes = unsafe { std::slice::from_raw_parts(argument.cast::<u8>(), DUMP_LEN) };

        append(&format!("  {name} first {DUMP_LEN} bytes: {bytes:02x?}"));

        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();

        append(&format!("  {name} as u32 words: {words:08x?}"));

        let pointers = words
            .iter()
            .filter(|word| looks_like_fn_ptr(**word))
            .count();

        append(&format!(
            "  {name}: {pointers}/{} values look like user mode pointers",
            words.len()
        ));
    }

    // `channels + 4` holds a pointer to a descriptor whose first DWORD is the
    // channel type.
    if !channels.is_null()
        && let Ok(descriptor) = usize::try_from(channels as isize)
        && let Some(descriptor) = read_pointer(descriptor + 4)
        && descriptor != 0
    {
        let channel_type = read_u32(descriptor).unwrap_or(0);
        append(&format!("  channel descriptor at 0x{descriptor:x}"));
        append(&format!("  channel type = {channel_type}"));

        let bytes = read_bytes(descriptor, DUMP_LEN).unwrap_or_default();
        append(&format!(
            "  descriptor first {DUMP_LEN} bytes: {bytes:02x?}"
        ));
    }

    // A zero is returned on purpose: the Sangfor client only expects a status
    // here and this build is not meant to provide a working channel.
    0
}

/// Reads a 32 bits little endian value, or [`None`] when the address is not
/// readable.
fn read_u32(address: usize) -> Option<u32> {
    let bytes = read_bytes(address, 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Reads a pointer stored at `address`, or [`None`] when the address is not
/// readable.
fn read_pointer(address: usize) -> Option<usize> {
    let bytes = read_bytes(address, 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
}

/// Reads `len` bytes, or [`None`] when the address is not readable.
fn read_bytes(address: usize, len: usize) -> Option<Vec<u8>> {
    if address == 0 || len == 0 {
        return None;
    }

    // SAFETY: the caller guarantees the address is mapped and readable.
    let slice = unsafe { std::slice::from_raw_parts(address as *const u8, len) };
    Some(slice.to_vec())
}

/// Sangfor static virtual channel plugin entry point.
///
/// # Safety
///
/// Every argument must either be null or point to at least [`DUMP_LEN`]
/// readable bytes, as provided by the Sangfor VDI client.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn svc_plugin_init(
    context: *mut c_void,
    channels: *mut c_void,
    _third: *mut c_void,
    _fourth: *mut c_void,
) -> i32 {
    unsafe { probe("svc_plugin_init", context, channels) }
}

/// Sangfor dynamic virtual channel plugin entry point.
///
/// # Safety
///
/// Every argument must either be null or point to at least [`DUMP_LEN`]
/// readable bytes, as provided by the Sangfor VDI client.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dvc_plugin_init(
    context: *mut c_void,
    channels: *mut c_void,
    _third: *mut c_void,
    _fourth: *mut c_void,
) -> i32 {
    unsafe { probe("dvc_plugin_init", context, channels) }
}
