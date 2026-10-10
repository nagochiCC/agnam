//! Local UnRAR processing boundary. The mutex covers complete native handle
//! lifetimes, including high-level listing, because UnRAR owns a global ErrHandler.
// Audited against unrar_sys 0.5.8's vendored dll.cpp (ProcessFile) and rdwrfn.cpp
// (UnpWrite): -1 throws inside C++ after callback return; RAR_TEST avoids native
// output writes. unpack50mt.cpp joins decode tasks before ProcessDecoded writes,
// so output callbacks do not concurrently borrow the Rust state. Keep the exact
// dependency pins until these contracts are checked again on an upgrade.
use crate::archive::resource::ResourceBudget;
use crate::error::AppError;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Mutex, MutexGuard};
use unrar_sys as sys;

static UNRAR: Mutex<()> = Mutex::new(());

pub(super) fn lock() -> MutexGuard<'static, ()> {
    UNRAR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn native_error(code: i32) -> AppError {
    AppError::Archive(format!("UnRAR error {code}"))
}

// Header/list callbacks do not receive an output sink. Reject password prompts,
// missing volumes and large dictionary overrides, preserving the existing policy.
extern "C" fn header_callback(
    msg: sys::UINT,
    _: sys::LPARAM,
    _: sys::LPARAM,
    p2: sys::LPARAM,
) -> i32 {
    match msg {
        sys::UCM_NEEDPASSWORD | sys::UCM_NEEDPASSWORDW => -1,
        sys::UCM_CHANGEVOLUME | sys::UCM_CHANGEVOLUMEW if p2 == sys::RAR_VOL_ASK => -1,
        _ => 0,
    }
}

pub(super) struct Entry {
    pub filename: PathBuf,
    pub size: u64,
    pub is_file: bool,
    redirection: bool,
}

pub(super) struct Archive {
    handle: NonNull<sys::Handle>,
    pub solid: bool,
    _guard: MutexGuard<'static, ()>,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let guard = lock();
        #[cfg(any(target_os = "linux", target_os = "netbsd"))]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_err(|_| AppError::Archive("RAR path contains NUL".into()))?
        };
        #[cfg(windows)]
        let name = {
            use std::os::windows::ffi::OsStrExt;
            let mut name: Vec<sys::WCHAR> = path.as_os_str().encode_wide().collect();
            if name.contains(&0) {
                return Err(AppError::Archive("RAR path contains NUL".into()));
            }
            name.push(0);
            name
        };
        #[cfg(not(any(target_os = "linux", target_os = "netbsd", windows)))]
        let name = {
            let name = path
                .to_str()
                .ok_or_else(|| AppError::Archive("Invalid RAR path".into()))?;
            if name.contains('\0') {
                return Err(AppError::Archive("RAR path contains NUL".into()));
            }
            name.chars()
                .map(|c| c as sys::WCHAR)
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        };
        let mut data = sys::OpenArchiveDataEx::new(name.as_ptr(), sys::RAR_OM_EXTRACT);
        data.callback = Some(header_callback);
        // SAFETY: name and data remain valid for this synchronous call; all
        // native calls are serialized. A non-null handle is owned until Drop.
        let raw = unsafe { sys::RAROpenArchiveEx(&mut data) };
        let handle =
            NonNull::new(raw.cast_mut()).ok_or_else(|| native_error(data.open_result as i32))?;
        let archive = Self {
            handle,
            solid: data.flags & sys::ROADF_SOLID != 0,
            _guard: guard,
        };
        if data.open_result != 0 {
            return Err(native_error(data.open_result as i32));
        }
        Ok(archive)
    }

    pub fn read_header(&mut self) -> Result<Option<Entry>, AppError> {
        let mut header = sys::HeaderDataEx::default();
        // SAFETY: exclusively owned live handle and initialized native header.
        let code = unsafe {
            sys::RARSetCallback(self.handle.as_ptr(), Some(header_callback), 0);
            sys::RARReadHeaderEx(self.handle.as_ptr(), &mut header)
        };
        if code == sys::ERAR_END_ARCHIVE {
            return Ok(None);
        }
        if code != sys::ERAR_SUCCESS {
            return Err(native_error(code));
        }
        let end = header
            .filename_w
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| AppError::Archive("RAR entry name is not terminated".into()))?;
        #[cfg(windows)]
        let filename = {
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_wide(&header.filename_w[..end]))
        };
        #[cfg(not(windows))]
        let filename = PathBuf::from(
            header.filename_w[..end]
                .iter()
                .map(|&c| {
                    u32::try_from(c)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| AppError::Archive("Invalid RAR entry name".into()))
                })
                .collect::<Result<String, _>>()?,
        );
        Ok(Some(Entry {
            filename,
            size: u64::from(header.unp_size) | (u64::from(header.unp_size_high) << 32),
            is_file: header.flags & sys::RHDF_DIRECTORY == 0,
            redirection: header.redir_type != 0,
        }))
    }

    pub fn skip(&mut self, entry: &Entry, budget: &mut ResourceBudget) -> Result<(), AppError> {
        budget.check_cancel()?;
        if self.solid && entry.is_file {
            // RAR_SKIP decodes solid dependencies without output callbacks.
            // RAR_TEST exposes those required bytes once, without retaining them.
            let limit = budget.limits().single_entry;
            self.process(entry, budget, limit, &mut std::io::sink(), None)?;
        } else {
            // SAFETY: current header belongs to this exclusively owned handle.
            let code = unsafe {
                sys::RARProcessFile(
                    self.handle.as_ptr(),
                    sys::RAR_SKIP,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if code != sys::ERAR_SUCCESS {
                return Err(native_error(code));
            }
        }
        Ok(())
    }

    pub fn process(
        &mut self,
        entry: &Entry,
        budget: &mut ResourceBudget,
        limit: u64,
        writer: &mut dyn Write,
        temp_path: Option<&Path>,
    ) -> Result<u64, AppError> {
        budget.preflight_with_limit(entry.size, limit)?;
        // RAR_TEST must not turn reference/link metadata into apparently
        // completed payloads. Ordinary stored/compressed files are supported.
        if entry.redirection {
            return Err(AppError::Archive(
                "RAR link/reference entry is unsupported".into(),
            ));
        }
        let mut state = CallbackState {
            budget,
            writer,
            temp_path,
            limit,
            bytes: 0,
            error: None,
        };
        // SAFETY: state stays at this stack address through the synchronous
        // process call. No reference is accessed elsewhere until it returns.
        // The callback only borrows native buffers during that invocation.
        let code = unsafe {
            sys::RARSetCallback(
                self.handle.as_ptr(),
                Some(callback),
                &mut state as *mut _ as sys::LPARAM,
            );
            sys::RARProcessFile(
                self.handle.as_ptr(),
                sys::RAR_TEST,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        // Clear the borrowed pointer before state is dropped, on every outcome.
        unsafe {
            sys::RARSetCallback(self.handle.as_ptr(), Some(header_callback), 0);
        }
        if let Some(error) = state.error {
            return Err(error);
        }
        state.budget.check_cancel()?;
        if code != sys::ERAR_SUCCESS {
            return Err(native_error(code));
        }
        if state.bytes != entry.size {
            return Err(AppError::Archive("RAR entry size mismatch".into()));
        }
        Ok(state.bytes)
    }
}

impl Drop for Archive {
    fn drop(&mut self) {
        // SAFETY: the handle is closed once, before releasing the native mutex.
        unsafe {
            sys::RARCloseArchive(self.handle.as_ptr());
        }
    }
}

struct CallbackState<'a, 'cancel> {
    budget: &'a mut ResourceBudget<'cancel>,
    writer: &'a mut dyn Write,
    temp_path: Option<&'a Path>,
    limit: u64,
    bytes: u64,
    error: Option<AppError>,
}

extern "C" fn callback(
    msg: sys::UINT,
    userdata: sys::LPARAM,
    p1: sys::LPARAM,
    p2: sys::LPARAM,
) -> i32 {
    if msg != sys::UCM_PROCESSDATA {
        return header_callback(msg, userdata, p1, p2);
    }
    if userdata == 0 {
        return -1;
    }
    // SAFETY: userdata is installed only by process(), remains live and is
    // exclusively accessed by synchronous callbacks until RARProcessFile returns.
    let state = unsafe { &mut *(userdata as *mut CallbackState<'_, '_>) };
    if state.error.is_some() {
        return -1;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.budget.check_cancel()?;
        let len = usize::try_from(p2)
            .ok()
            .filter(|&len| len <= isize::MAX as usize)
            .ok_or_else(|| AppError::Archive("Invalid UnRAR callback length".into()))?;
        if len == 0 {
            return Ok(());
        }
        if p1 == 0 || (p1 as usize).checked_add(len).is_none() {
            return Err(AppError::Archive("Null UnRAR callback buffer".into()));
        }
        state
            .budget
            .account_chunk(&mut state.bytes, len as u64, state.limit, state.temp_path)?;
        // SAFETY: UnRAR supplies len initialized bytes for this callback only.
        let bytes = unsafe { std::slice::from_raw_parts(p1 as *const u8, len) };
        state.writer.write_all(bytes).map_err(AppError::from)
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            state.error = Some(error);
            -1
        }
        Err(payload) => {
            // A user panic payload may itself panic when dropped.
            std::mem::forget(payload);
            state.error = Some(AppError::Archive("UnRAR callback panicked".into()));
            -1
        }
    }
}

#[cfg(test)]
fn crc(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

// Test-only stored RAR4 builder: exercises real native output without a RAR
// writer executable or large binary fixtures.
#[cfg(test)]
pub(in crate::archive) fn stored_rar(entries: &[(&str, &[u8])]) -> Vec<u8> {
    fn header(out: &mut Vec<u8>, kind: u8, flags: u16, payload: &[u8]) {
        let mut header = vec![kind];
        header.extend_from_slice(&flags.to_le_bytes());
        header.extend_from_slice(&((7 + payload.len()) as u16).to_le_bytes());
        header.extend_from_slice(payload);
        out.extend_from_slice(&(crc(&header) as u16).to_le_bytes());
        out.extend_from_slice(&header);
    }
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    header(&mut out, 0x73, 0, &[0; 6]);
    for (name, bytes) in entries {
        let mut payload = Vec::new();
        payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        payload.push(2); // Windows host, regular file.
        payload.extend_from_slice(&crc(bytes).to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&[20, 0x30]); // Stored method.
        payload.extend_from_slice(&(name.len() as u16).to_le_bytes());
        payload.extend_from_slice(&0x20u32.to_le_bytes());
        payload.extend_from_slice(name.as_bytes());
        header(&mut out, 0x74, 0x8000, &payload);
        out.extend_from_slice(bytes);
    }
    header(&mut out, 0x7b, 0, &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::resource::ResourceLimits;
    use crate::archive::{ProgressiveArchiveCancelToken, ResourceLimitKind};

    fn new_budget(single: u64, cumulative: u64) -> ResourceBudget<'static> {
        ResourceBudget::new(ResourceLimits {
            single_entry: single,
            cumulative,
            temp_writes: cumulative,
            temp_occupancy: cumulative,
            entries: 4096,
            images: 4096,
        })
    }

    fn fixture(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file
    }

    #[test]
    fn disk_materialization_crosses_the_real_64_mib_boundary() {
        // Generate the payload with a fixed buffer, not a 65 MiB Vec/fixture.
        // This tests the materialization boundary; the dummy nested payload is
        // not used as a readable descendant (the mixed-backend tests cover that).
        let size = 65 * crate::archive::resource::MIB;
        let mut data = stored_rar(&[("inner.rar", &[])]);
        data[27..31].copy_from_slice(&(size as u32).to_le_bytes());
        data[31..35].copy_from_slice(&(size as u32).to_le_bytes());
        // CRC32 of exactly 65 MiB of zeros; native completion verifies it.
        data[36..40].copy_from_slice(&0x97d38551u32.to_le_bytes());
        let entry_end = 20 + u16::from_le_bytes(data[25..27].try_into().unwrap()) as usize;
        let header_crc = crc(&data[22..entry_end]) as u16;
        data[20..22].copy_from_slice(&header_crc.to_le_bytes());
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&data[..entry_end]).unwrap();
        for _ in 0..size / 65536 {
            file.write_all(&[0; 65536]).unwrap();
        }
        file.write_all(&data[entry_end..]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut budget = ResourceBudget::default();
        super::super::rar::extract_to_dir(file.path(), directory.path(), &mut budget).unwrap();
        assert_eq!(
            std::fs::metadata(directory.path().join("inner.rar"))
                .unwrap()
                .len(),
            size
        );
        assert_eq!(budget.temp_counters(), (size, size));
        // The bytes API has no disk-materialization exemption, even for .rar.
        assert!(matches!(
            super::super::rar::automatic_cover_entry_bytes(
                file.path(),
                Path::new("inner.rar"),
                &mut ResourceBudget::default()
            ),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::SingleEntryBytes
            ))
        ));
    }

    #[test]
    fn native_processing_rejects_traversal_and_corrupt_headers() {
        for name in ["../inner.rar", "/inner.rar"] {
            let file = fixture(&stored_rar(&[(name, b"payload")]));
            let directory = tempfile::tempdir().unwrap();
            let mut budget = new_budget(1024, 4096);
            assert!(
                super::super::rar::extract_to_dir(file.path(), directory.path(), &mut budget)
                    .is_err(),
                "{name}"
            );
            assert_eq!(budget.temp_counters(), (0, 0));
        }
        let mut data = stored_rar(&[("inner.rar", b"payload")]);
        data[20] ^= 1; // File header CRC, not the output CRC.
        let file = fixture(&data);
        let directory = tempfile::tempdir().unwrap();
        assert!(
            super::super::rar::extract_to_dir(
                file.path(),
                directory.path(),
                &mut new_budget(1024, 4096)
            )
            .is_err()
        );
        let output = directory.path().join("inner.rar");
        // Earlier completed entries are owned by the caller's workspace; the
        // failed archive tree is discarded, never returned as a Document.
        drop(directory);
        assert!(!output.exists());
    }

    #[test]
    fn stored_rar_disk_quota_crc_failure_and_partial_cleanup() {
        let data = stored_rar(&[("inner.rar", b"12345678")]);
        for (corrupt, quota) in [(false, 8), (false, 7), (true, 8)] {
            let mut data = data.clone();
            if corrupt {
                let pos = data
                    .windows(8)
                    .position(|window| window == b"12345678")
                    .unwrap();
                data[pos] ^= 1;
            }
            let file = fixture(&data);
            let directory = tempfile::tempdir().unwrap();
            let mut budget = new_budget(4, 16);
            // The disk allowance is independent from the cumulative limit.
            budget = ResourceBudget::new(ResourceLimits {
                temp_writes: quota,
                temp_occupancy: 16,
                ..budget.limits()
            });
            let result =
                super::super::rar::extract_to_dir(file.path(), directory.path(), &mut budget);
            let path = directory.path().join("inner.rar");
            if !corrupt && quota == 8 {
                result.unwrap();
                assert_eq!(std::fs::read(&path).unwrap(), b"12345678");
            } else {
                if corrupt {
                    assert!(matches!(result, Err(AppError::Archive(_))));
                } else {
                    assert!(matches!(
                        result,
                        Err(AppError::ArchiveResourceLimit(
                            ResourceLimitKind::TemporaryWrites
                        ))
                    ));
                }
                assert!(!path.exists());
                assert_eq!(budget.temp_counters().1, 0);
            }
        }
    }

    #[test]
    fn required_solid_skip_is_charged_and_can_hit_the_operation_limit() {
        // unrar 0.5.8 data/solid.rar (MIT OR Apache-2.0), retained verbatim.
        let file = fixture(include_bytes!("fixtures/rar5-solid.rar"));
        let mut archive = Archive::open(file.path()).unwrap();
        assert!(archive.solid);
        let first = archive.read_header().unwrap().unwrap();
        let mut budget = new_budget(1024, first.size);
        archive.skip(&first, &mut budget).unwrap();
        // The small upstream solid fixture has one entry. The dependency
        // decode consumed the operation allowance even though nothing was kept.
        assert!(matches!(
            budget.account_chunk(&mut 0, 1, 1024, None),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::CumulativeBytes
            ))
        ));
        assert_eq!(budget.temp_counters(), (0, 0));
    }

    #[test]
    fn solid_dependencies_and_selected_payload_share_one_exact_budget() {
        let file = fixture(include_bytes!("fixtures/rar5-multiple-solid.rar"));
        for allowance in [3 * 4096, 3 * 4096 - 1] {
            let mut archive = Archive::open(file.path()).unwrap();
            assert!(archive.solid);
            let mut budget = new_budget(4096, allowance);
            for _ in 0..2 {
                let entry = archive.read_header().unwrap().unwrap();
                archive.skip(&entry, &mut budget).unwrap();
            }
            let selected = archive.read_header().unwrap().unwrap();
            assert_eq!(selected.filename, Path::new("test3.bin"));
            let mut bytes = Vec::new();
            let result = archive.process(&selected, &mut budget, 4096, &mut bytes, None);
            if allowance == 3 * 4096 {
                result.unwrap(); // Native CRC validates the dependency decoder state.
                assert_eq!(bytes.len(), 4096);
            } else {
                assert!(matches!(
                    result,
                    Err(AppError::ArchiveResourceLimit(
                        ResourceLimitKind::CumulativeBytes
                    ))
                ));
            }
            assert_eq!(budget.temp_counters(), (0, 0));
        }
    }

    #[test]
    fn rar4_and_rar5_decode_and_close_before_reopening() {
        for bytes in [
            stored_rar(&[("page.png", b"image")]),
            include_bytes!("fixtures/rar5-solid.rar").to_vec(),
            super::super::rar::tests::RAR_WITH_COVER.to_vec(),
        ] {
            let file = fixture(&bytes);
            for _ in 0..2 {
                let mut archive = Archive::open(file.path()).unwrap();
                let mut budget = new_budget(1024, 4096);
                let mut count = 0;
                while let Some(entry) = archive.read_header().unwrap() {
                    let mut out = Vec::new();
                    archive
                        .process(&entry, &mut budget, 1024, &mut out, None)
                        .unwrap();
                    assert_eq!(out.len() as u64, entry.size);
                    count += 1;
                }
                assert!(count > 0);
            }
        }
    }

    #[test]
    fn quota_aborts_native_callback_before_disk_write_and_releases_handle() {
        let file = fixture(super::super::rar::tests::RAR_WITH_COVER);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial");
        let mut archive = Archive::open(file.path()).unwrap();
        let mut entry = archive.read_header().unwrap().unwrap();
        let actual = entry.size;
        // Exercise actual output enforcement despite a smaller preflight value.
        entry.size = 1;
        let mut budget = new_budget(1, 4096);
        let mut writer = std::fs::File::create(&path).unwrap();
        budget.begin_temp_file(&path).unwrap();
        assert!(matches!(
            archive.process(&entry, &mut budget, 1, &mut writer, Some(&path)),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::SingleEntryBytes
            ))
        ));
        assert_eq!(writer.metadata().unwrap().len(), 0);
        drop(writer);
        drop(archive);
        let mut archive = Archive::open(file.path()).unwrap();
        let entry = archive.read_header().unwrap().unwrap();
        let mut budget = new_budget(actual, actual);
        let mut out = Vec::new();
        archive
            .process(&entry, &mut budget, actual, &mut out, None)
            .unwrap();
        assert_eq!(out.len() as u64, actual);
        drop(directory);
        assert!(!path.exists());
    }

    #[test]
    fn callback_propagates_partial_io_failure_and_cancel() {
        struct FailingWriter {
            written: usize,
            cancel: Option<ProgressiveArchiveCancelToken>,
        }
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.written != 0 {
                    return Err(std::io::ErrorKind::StorageFull.into());
                }
                self.written += 1;
                if let Some(cancel) = &self.cancel {
                    cancel.cancel();
                }
                Ok(bytes.len().min(1))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let file = fixture(super::super::rar::tests::RAR_WITH_COVER);
        for cancel in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let output = directory.path().join("partial.rar");
            let token = ProgressiveArchiveCancelToken::default();
            let mut archive = Archive::open(file.path()).unwrap();
            let entry = archive.read_header().unwrap().unwrap();
            let mut budget = new_budget(1024, 4096);
            budget.set_cancel_token(&token);
            budget.begin_temp_file(&output).unwrap();
            let mut writer = FailingWriter {
                written: 0,
                cancel: cancel.then_some(token.clone()),
            };
            let result = archive.process(&entry, &mut budget, 1024, &mut writer, Some(&output));
            if cancel {
                assert!(token.is_cancelled());
            }
            assert!(
                matches!(result, Err(AppError::Io(ref error)) if error.kind() == std::io::ErrorKind::StorageFull)
            );
            assert_eq!(writer.written, 1);
            // Reserve the entire callback even when only one byte was written.
            assert_eq!(budget.temp_counters(), (entry.size, entry.size));
            drop(directory);
            budget.release_temp_tree(&output).unwrap();
            assert_eq!(budget.temp_counters(), (entry.size, 0));
        }
        // Cancel issued inside the first output callback must stop at the next
        // callback or before successful completion, without adopting the result.
        struct CancelWriter(ProgressiveArchiveCancelToken);
        impl Write for CancelWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.cancel();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let token = ProgressiveArchiveCancelToken::default();
        let mut budget = new_budget(1024, 4096);
        budget.set_cancel_token(&token);
        let mut archive = Archive::open(file.path()).unwrap();
        let entry = archive.read_header().unwrap().unwrap();
        assert!(matches!(
            archive.process(&entry, &mut budget, 1024, &mut CancelWriter(token), None),
            Err(AppError::ArchiveCancelled)
        ));
    }

    #[test]
    fn callback_checks_pointers_and_catches_writer_panics() {
        struct PanicWriter;
        impl Write for PanicWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                panic!("injected writer panic")
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut budget = new_budget(4, 4);
        let mut state = CallbackState {
            budget: &mut budget,
            writer: &mut PanicWriter,
            temp_path: None,
            limit: 4,
            bytes: 0,
            error: None,
        };
        let ptr = &mut state as *mut _ as sys::LPARAM;
        assert_eq!(callback(sys::UCM_PROCESSDATA, ptr, 0, 0), 0);
        assert_eq!(callback(sys::UCM_PROCESSDATA, ptr, 0, 1), -1);
        assert!(state.error.is_some());
        state.error = None;
        assert_eq!(
            callback(sys::UCM_PROCESSDATA, ptr, b"x".as_ptr() as sys::LPARAM, 1),
            -1
        );
        assert!(
            matches!(&state.error, Some(AppError::Archive(message)) if message.contains("panicked"))
        );
        let file = fixture(super::super::rar::tests::RAR_WITH_COVER);
        let mut archive = Archive::open(file.path()).unwrap();
        let entry = archive.read_header().unwrap().unwrap();
        assert!(matches!(
            archive.process(&entry, &mut new_budget(1024, 4096), 1024, &mut PanicWriter, None),
            Err(AppError::Archive(message)) if message.contains("panicked")
        ));
        drop(archive);
        Archive::open(file.path()).unwrap();
    }

    #[test]
    fn concurrent_native_handles_are_serialized() {
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    let file = fixture(super::super::rar::tests::RAR_WITH_COVER);
                    assert!(super::super::rar::has_viewable_content(file.path()).unwrap());
                    assert!(!super::super::rar::has_nested_archives(file.path()));
                    let mut archive = Archive::open(file.path()).unwrap();
                    let entry = archive.read_header().unwrap().unwrap();
                    archive
                        .process(
                            &entry,
                            &mut new_budget(1024, 4096),
                            1024,
                            &mut Vec::new(),
                            None,
                        )
                        .unwrap();
                });
            }
        });
    }
}
