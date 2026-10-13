//! Registration of the soxy frontend inside the Sangfor VDI client.
//!
//! Contrary to `VMware` Horizon, `FreeRDP`, Remmina, Citrix and the Microsoft RDP
//! client, the Sangfor VDI client exposes no registry key and no configuration
//! file to declare an additional RDP virtual channel plugin: the list of channel
//! plugin DLLs is hardcoded, as NUL terminated strings, inside its own binaries
//! (`RVLSession.exe`, `SRAPSession*.exe`, `libsrapc*.dll`, ...).
//!
//! `soxyreg sangfor` works around this by overwriting one entry of that
//! hardcoded list with the file name of the soxy frontend DLL. Only the bytes of
//! the selected string are modified: the replacement is never longer than the
//! original one and the remaining bytes are filled with NULs, so no pointer,
//! offset or relocation is touched. A byte exact backup of every modified file
//! is kept next to it and `soxyreg sangfor unregister` restores it.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Suffix appended to the name of a modified client binary to store its
/// unmodified content.
const BACKUP_SUFFIX: &str = "soxy.bak";

/// Registry key holding the installation path of the Sangfor VDI client.
const REG_PATH: &str = r"SOFTWARE\Sangfor\VDI\RemoteAppClient";

/// Value of [`REG_PATH`] containing the installation directory.
const REG_VALUE_PATH: &str = "Path";

/// Fallback installation directory of the Sangfor VDI client.
const DEFAULT_DIR: &str = r"C:\Program Files (x86)\Sangfor\VDI\RemoteAppClient";

/// Binaries of the Sangfor VDI client which embed the hardcoded channel plugin
/// list, in the order they are inspected.
const CLIENT_BINARIES: &[&str] = &[
    "RVLSession.exe",
    "SRAPSession.exe",
    "SRAPSession2.exe",
    "SfRemoteAppSession.exe",
    "SfRemoteAppClient.exe",
    "libsrapc.dll",
    "libsrapc2.dll",
    "libngs.dll",
    "libngs2.dll",
];

/// Channel plugin names hardcoded in the Sangfor VDI client, ordered from the
/// least to the most useful feature to give up when one of them is replaced by
/// the soxy frontend.
const KNOWN_PLUGINS: &[&str] = &[
    "rvlprinter.dll",
    "librdpgfx.dll",
    "libusbtray.dll",
    "librdpsnd.dll",
    "librdpclip.dll",
    "librdpdr.dll",
    "librvldr.dll",
    "librvlclip.dll",
    "librvlmmr.dll",
    "librvldisk.dll",
    "librvlpar.dll",
    "librdpclip2.dll",
    "librdpsnd2.dll",
    "librdpdr2.dll",
];

/// Maximum distance, in bytes, between two consecutive entries of the hardcoded
/// plugin table.
const TABLE_WINDOW: usize = 64;

/// Minimum number of plugin names which must surround an occurrence for it to
/// be considered as part of the hardcoded plugin table. This rejects the
/// occurrences found in log messages and error strings, which mention a plugin
/// file name without being part of the table itself.
const MIN_NEIGHBORS: usize = 2;

/// A NUL terminated plugin file name found inside a client binary.
struct Slot {
    /// Offset of the first byte of the plugin file name.
    offset: u64,
    /// Length of the plugin file name, excluding its NUL terminator.
    len: usize,
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    name.push('.');
    name.push_str(BACKUP_SUFFIX);
    path.with_file_name(name)
}

/// Expands the `%VARIABLE%` sequences of a registry value.
fn expand_env(value: &str) -> String {
    let mut expanded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let tail = &rest[start + 1..];

        match tail.find('%') {
            Some(end) if !tail[..end].is_empty() => {
                let name = &tail[..end];
                expanded.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
                rest = &tail[end + 1..];
            }
            _ => {
                expanded.push('%');
                rest = tail;
            }
        }
    }

    expanded.push_str(rest);
    expanded
}

/// Returns the installation directory of the Sangfor VDI client.
fn install_root() -> Result<PathBuf, String> {
    let hklm = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);

    let candidates = [
        REG_PATH.to_string(),
        format!(r"SOFTWARE\WOW6432Node\{REG_PATH}"),
    ];

    for candidate in candidates {
        if let Ok(key) = hklm.open_subkey(&candidate)
            && let Ok(value) = key.get_value::<String, _>(REG_VALUE_PATH)
        {
            let expanded = expand_env(&value);
            let path = PathBuf::from(expanded.trim());

            if path.is_dir() {
                return Ok(path);
            }
        }
    }

    let fallback = PathBuf::from(DEFAULT_DIR);

    if fallback.is_dir() {
        return Ok(fallback);
    }

    Err(format!(
        "unable to locate the Sangfor VDI client: neither {REG_PATH}\\{REG_VALUE_PATH} nor {DEFAULT_DIR} exist"
    ))
}

/// Returns the offsets of every hardcoded plugin name of the binary.
fn plugin_offsets(data: &[u8]) -> Vec<usize> {
    let mut offsets = Vec::new();

    for plugin in KNOWN_PLUGINS {
        let needle = plugin.as_bytes();
        let mut from = 0;

        while let Some(found) = data[from..]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            let offset = from + found;
            from = offset + 1;
            offsets.push(offset);
        }
    }

    offsets
}

/// Returns the NUL terminated occurrences of `name` inside `data`.
fn slots(data: &[u8], name: &str) -> Vec<Slot> {
    let needle = name.as_bytes();
    let mut slots = Vec::new();
    let mut from = 0;

    while let Some(found) = data[from..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let offset = from + found;
        from = offset + 1;

        let start = offset;
        let end = offset + needle.len();

        // The name must be a complete NUL terminated string.
        if data.get(end) != Some(&0) {
            continue;
        }

        // ... and must not be the tail of a longer string.
        if start > 0 && data[start - 1] != 0 {
            continue;
        }

        if let Ok(offset) = u64::try_from(start) {
            slots.push(Slot {
                offset,
                len: needle.len(),
            });
        }
    }

    slots
}

/// Returns the occurrence of `victim` which belongs to the hardcoded plugin
/// table, or [`None`] when `victim` is only mentioned in strings which are not
/// the table itself (log messages, error strings, ...).
///
/// An occurrence belongs to the table when it is surrounded by at least
/// [`MIN_NEIGHBORS`] other known plugin names within [`TABLE_WINDOW`] bytes.
fn best_slot(offsets: &[usize], data: &[u8], victim: &str) -> Option<Slot> {
    let neighbors = |slot: &Slot| {
        let center = usize::try_from(slot.offset).unwrap_or_default();
        offsets
            .iter()
            .filter(|offset| offset.abs_diff(center) <= TABLE_WINDOW)
            .count()
    };

    slots(data, victim)
        .into_iter()
        .filter(|slot| neighbors(slot) >= MIN_NEIGHBORS)
        .max_by_key(neighbors)
}

/// Checks that `victim` is one of the channel plugins known to be hardcoded in
/// the Sangfor VDI client.
fn check_victim(victim: &str) -> Result<(), String> {
    if KNOWN_PLUGINS.contains(&victim) {
        return Ok(());
    }

    Err(format!(
        "unknown Sangfor channel plugin {victim}; expected one of {}",
        KNOWN_PLUGINS.join(", ")
    ))
}

/// Returns the plugin name which is going to be replaced, and the location of
/// its string inside the binary.
///
/// When `requested` is given, it is the only candidate; otherwise the least
/// useful plugin of the binary is selected.
fn select_victim(data: &[u8], requested: Option<&str>) -> Option<(String, Slot)> {
    let offsets = plugin_offsets(data);

    if let Some(requested) = requested {
        return best_slot(&offsets, data, requested).map(|slot| (requested.to_string(), slot));
    }

    KNOWN_PLUGINS.iter().find_map(|plugin| {
        best_slot(&offsets, data, plugin).map(|slot| ((*plugin).to_string(), slot))
    })
}

/// Copies the soxy frontend DLL into the Sangfor VDI client directory.
fn install_dll(dll: &Path, root: &Path, dry_run: bool) -> Result<PathBuf, String> {
    let file_name = dll
        .file_name()
        .ok_or_else(|| format!("invalid DLL path {}", dll.display()))?;
    let destination = root.join(file_name);

    if !dll.is_file() {
        return Err(format!("{} is not a file", dll.display()));
    }

    if destination.exists() && same_content(dll, &destination)? {
        return Ok(destination);
    }

    if dry_run {
        println!(
            "  [dry-run] would copy {} to {}",
            dll.display(),
            destination.display()
        );
        return Ok(destination);
    }

    fs::copy(dll, &destination).map_err(|e| {
        format!(
            "failed to copy {} into the client directory: {e}",
            dll.display()
        )
    })?;

    Ok(destination)
}

fn same_content(left: &Path, right: &Path) -> Result<bool, String> {
    let left = fs::read(left).map_err(|e| format!("failed to read {}: {e}", left.display()))?;
    let right = fs::read(right).map_err(|e| format!("failed to read {}: {e}", right.display()))?;

    Ok(left == right)
}

/// Returns the plugin file name currently stored at `slot`, without the NUL
/// padding left by a previous patch.
fn name_at(data: &[u8], slot: &Slot) -> String {
    let start = usize::try_from(slot.offset).unwrap_or_default();
    let end = start.saturating_add(slot.len).min(data.len());
    let raw = data.get(start..end).unwrap_or_default();

    let len = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());

    String::from_utf8_lossy(&raw[..len]).into_owned()
}

/// Overwrites the selected hardcoded plugin name with `dll_name`.
fn write_slot(path: &Path, slot: &Slot, dll_name: &str, dry_run: bool) -> Result<(), String> {
    let mut replacement = Vec::with_capacity(slot.len);
    replacement.extend_from_slice(dll_name.as_bytes());
    replacement.resize(slot.len, 0);

    if dry_run {
        println!(
            "  [dry-run] would write {replacement:?} at offset {}",
            slot.offset
        );
        return Ok(());
    }

    let backup = backup_path(path);

    if !backup.exists() {
        fs::copy(path, &backup).map_err(|e| format!("failed to backup {}: {e}", path.display()))?;
        println!("  backup created: {}", backup.display());
    }

    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("failed to open {} for writing: {e}", path.display()))?;

    file.seek(SeekFrom::Start(slot.offset))
        .map_err(|e| format!("failed to seek in {}: {e}", path.display()))?;

    file.write_all(&replacement)
        .map_err(|e| format!("failed to patch {}: {e}", path.display()))?;

    file.flush()
        .map_err(|e| format!("failed to flush {}: {e}", path.display()))?;

    // Read the patched string back to make sure the client will load the
    // expected DLL.
    let mut check = vec![0; slot.len];
    file.seek(SeekFrom::Start(slot.offset))
        .map_err(|e| format!("failed to seek in {}: {e}", path.display()))?;
    file.read_exact(&mut check)
        .map_err(|e| format!("failed to verify {}: {e}", path.display()))?;

    let expected = &replacement[..dll_name.len()];

    if check[..dll_name.len()] != *expected || check[dll_name.len()..].iter().any(|b| *b != 0) {
        return Err(format!(
            "verification failed after patching {}",
            path.display()
        ));
    }

    Ok(())
}

/// Restores the original content of a patched client binary.
fn restore(path: &Path, dry_run: bool) -> Result<bool, String> {
    let backup = backup_path(path);

    if !backup.exists() {
        return Ok(false);
    }

    if dry_run {
        println!(
            "  [dry-run] would restore {} from {}",
            path.display(),
            backup.display()
        );
        return Ok(true);
    }

    fs::copy(&backup, path).map_err(|e| format!("failed to restore {}: {e}", path.display()))?;

    fs::remove_file(&backup).map_err(|e| format!("failed to remove {}: {e}", backup.display()))?;

    Ok(true)
}

fn read_binary(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))
}

fn client_paths(root: &Path) -> Vec<PathBuf> {
    CLIENT_BINARIES
        .iter()
        .map(|name| root.join(name))
        .filter(|path| path.is_file())
        .collect()
}

/// Registers the soxy frontend DLL inside the Sangfor VDI client.
pub fn register(dll: &str, victim: Option<&str>, dry_run: bool) {
    println!("Sangfor VDI client registration");

    let root = match install_root() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("error: {e}");
            return;
        }
    };

    println!("client directory = {}", root.display());

    let dll = PathBuf::from(dll);

    if !dll.is_file() {
        eprintln!("error: {} is not a file", dll.display());
        return;
    }

    if let Some(victim) = victim
        && let Err(e) = check_victim(victim)
    {
        eprintln!("error: {e}");
        return;
    }

    let installed = match install_dll(&dll, &root, dry_run) {
        Ok(installed) => installed,
        Err(e) => {
            eprintln!("error: {e}");
            return;
        }
    };

    let dll_name = installed
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());

    println!("frontend DLL     = {}", installed.display());

    let mut patched = 0;

    for path in client_paths(&root) {
        println!("inspecting {}", path.display());

        let data = match read_binary(&path) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("  error: {e}");
                continue;
            }
        };

        let Some((victim_name, slot)) = select_victim(&data, victim) else {
            if victim.is_some() {
                println!("  no reference to the requested channel plugin");
            } else {
                println!("  no hardcoded channel plugin list found");
            }
            continue;
        };

        if dll_name.len() > slot.len {
            eprintln!(
                "  error: {dll_name:?} is longer than {victim_name:?}, pick a shorter file name"
            );
            continue;
        }

        println!("  replacing {victim_name:?} by {dll_name:?}");

        if let Err(e) = write_slot(&path, &slot, &dll_name, dry_run) {
            eprintln!("  error: {e}");
            continue;
        }

        patched += 1;
    }

    if patched == 0 {
        eprintln!("error: no client binary has been patched");
        return;
    }

    if dry_run {
        println!("dry-run: {patched} binary(ies) would be patched");
    } else {
        println!("{patched} binary(ies) patched");
        println!("note: the Sangfor VDI client must be restarted for the change to take effect");
    }
}

/// Unregisters the soxy frontend DLL from the Sangfor VDI client.
pub fn unregister(dry_run: bool) {
    println!("Sangfor VDI client unregistration");

    let root = match install_root() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("error: {e}");
            return;
        }
    };

    println!("client directory = {}", root.display());

    let mut restored = 0;

    for path in client_paths(&root) {
        if !backup_path(&path).exists() {
            continue;
        }

        println!("restoring {}", path.display());

        match restore(&path, dry_run) {
            Ok(true) => restored += 1,
            Ok(false) => (),
            Err(e) => eprintln!("  error: {e}"),
        }
    }

    if restored == 0 {
        println!("nothing to restore");
    } else if dry_run {
        println!("dry-run: {restored} binary(ies) would be restored");
    } else {
        println!("{restored} binary(ies) restored");
    }
}

/// Reports the current state of the Sangfor VDI client registration.
pub fn status() {
    println!("Sangfor VDI client status");

    let root = match install_root() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("error: {e}");
            return;
        }
    };

    println!("client directory = {}", root.display());

    for path in client_paths(&root) {
        let patched = backup_path(&path).exists();
        let data = match read_binary(&path) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("{}: error: {e}", path.display());
                continue;
            }
        };

        let mut loaded = Vec::new();

        for plugin in KNOWN_PLUGINS {
            for slot in slots(&data, plugin) {
                let current = name_at(&data, &slot);

                if current != *plugin {
                    loaded.push(format!("{plugin} -> {current}"));
                }
            }
        }

        println!(
            "{}: {}{}",
            path.display(),
            if patched { "patched" } else { "original" },
            if loaded.is_empty() {
                String::new()
            } else {
                format!(" ({})", loaded.join(", "))
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A binary containing a hardcoded plugin table, preceded by a log message
    /// which mentions one of the plugin names.
    fn table() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"error: failed to load librdpclip.dll\0");
        data.extend_from_slice(b"librdpsnd.dll\0");
        data.extend_from_slice(b"librdpclip.dll\0");
        data.extend_from_slice(b"librdpdr.dll\0");
        data.extend_from_slice(b"unsupported operation\0");
        data
    }

    #[test]
    fn slots_only_contains_complete_strings() {
        let data = table();
        // the occurrence of the log message is the tail of a longer string and
        // is therefore not a plugin name on its own
        let found = slots(&data, "librdpclip.dll");

        assert_eq!(found.len(), 1);
        for slot in &found {
            let start = usize::try_from(slot.offset).unwrap();
            assert_eq!(&data[start..start + slot.len], b"librdpclip.dll");
            assert_eq!(data[start + slot.len], 0);
        }
    }

    #[test]
    fn best_slot_selects_the_table_entry() {
        let data = table();
        let offsets = plugin_offsets(&data);
        let slot = best_slot(&offsets, &data, "librdpclip.dll").expect("a slot");

        let start = usize::try_from(slot.offset).unwrap();
        assert_eq!(&data[start..start + slot.len], b"librdpclip.dll");
        // the entry of the table, not the one of the log message
        assert!(start > 30);
    }

    #[test]
    fn best_slot_rejects_strings_outside_of_the_table() {
        let data = b"warning: librdpgfx.dll is missing\0";
        let offsets = plugin_offsets(data);

        assert!(best_slot(&offsets, data, "librdpgfx.dll").is_none());
    }

    #[test]
    fn select_victim_prefers_the_least_useful_plugin() {
        let data = table();
        let (victim, _) = select_victim(&data, None).expect("a victim");

        assert_eq!(victim, "librdpsnd.dll");
    }

    #[test]
    fn name_at_drops_the_nul_padding() {
        let mut data = table();
        let offsets = plugin_offsets(&data);
        let slot = best_slot(&offsets, &data, "librdpclip.dll").unwrap();

        let start = usize::try_from(slot.offset).unwrap();
        assert_eq!(name_at(&data, &slot), "librdpclip.dll");

        data[start..start + slot.len].copy_from_slice(b"soxy.dll\0\0\0\0\0\0");
        assert_eq!(name_at(&data, &slot), "soxy.dll");
    }

    #[test]
    fn check_victim_rejects_unknown_plugins() {
        assert!(check_victim("rvlprinter.dll").is_ok());
        assert!(check_victim("soxy.dll").is_err());
    }

    #[test]
    fn expand_env_replaces_variables() {
        // SAFETY: the variable is removed at the end of the test.
        unsafe { std::env::set_var("SOXY_TEST_DIR", r"C:\soxy") };

        assert_eq!(expand_env(r"%SOXY_TEST_DIR%\client"), r"C:\soxy\client");
        assert_eq!(expand_env(r"C:\plain"), r"C:\plain");
        assert_eq!(expand_env(r"%SOXY_UNKNOWN%\x"), r"%SOXY_UNKNOWN%\x");

        // SAFETY: the variable is not used anymore.
        unsafe { std::env::remove_var("SOXY_TEST_DIR") };
    }

    #[test]
    fn patch_and_restore_round_trip() {
        let path = std::env::temp_dir().join("soxyreg-sangfor-test.bin");
        let original = table();

        std::fs::write(&path, &original).unwrap();

        let data = std::fs::read(&path).unwrap();
        let offsets = plugin_offsets(&data);
        let slot = best_slot(&offsets, &data, "librdpclip.dll").unwrap();
        let start = usize::try_from(slot.offset).unwrap();

        write_slot(&path, &slot, "soxy.dll", false).unwrap();

        let patched = std::fs::read(&path).unwrap();

        assert_eq!(&patched[start..start + 8], b"soxy.dll");
        assert!(patched[start + 8..start + slot.len].iter().all(|b| *b == 0));
        // only the bytes of the selected string have been modified
        assert_eq!(&patched[..start], &original[..start]);
        assert_eq!(&patched[start + slot.len..], &original[start + slot.len..]);
        assert_eq!(patched.len(), original.len());

        assert!(restore(&path, false).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(!backup_path(&path).exists());

        let _ = std::fs::remove_file(&path);
    }
}
