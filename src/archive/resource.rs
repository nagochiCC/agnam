use crate::error::AppError;
use std::collections::HashMap;
use std::io::{Read, Write};

pub(crate) const MIB: u64 = 1024 * 1024;
pub(crate) const SINGLE_ENTRY_LIMIT: u64 = 64 * MIB;
pub(crate) const OPERATION_LIMIT: u64 = 512 * MIB;
pub(crate) const TEMP_WRITE_LIMIT: u64 = 512 * MIB;
pub(crate) const TEMP_OCCUPANCY_LIMIT: u64 = 512 * MIB;
pub(crate) const ENTRY_LIMIT: u64 = 4096;
pub(crate) const IMAGE_ENTRY_LIMIT: u64 = 4096;
const IO_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ResourceLimitKind {
    #[error("single archive entry exceeds its byte limit")]
    SingleEntryBytes,
    #[error("operation exceeds its cumulative byte limit")]
    CumulativeBytes,
    #[error("temporary disk cumulative write limit exceeded")]
    TemporaryWrites,
    #[error("temporary disk occupancy limit exceeded")]
    TemporaryOccupancy,
    #[error("archive entry count limit exceeded")]
    EntryCount,
    #[error("archive image entry count limit exceeded")]
    ImageEntryCount,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResourceLimits {
    pub(crate) single_entry: u64,
    pub(crate) cumulative: u64,
    pub(crate) temp_writes: u64,
    pub(crate) temp_occupancy: u64,
    pub(crate) entries: u64,
    pub(crate) images: u64,
}

impl ResourceLimits {
    pub(crate) const PRODUCTION: Self = Self {
        single_entry: SINGLE_ENTRY_LIMIT,
        cumulative: OPERATION_LIMIT,
        temp_writes: TEMP_WRITE_LIMIT,
        temp_occupancy: TEMP_OCCUPANCY_LIMIT,
        entries: ENTRY_LIMIT,
        images: IMAGE_ENTRY_LIMIT,
    };
}

#[derive(Debug)]
pub(crate) struct ResourceBudget {
    limits: ResourceLimits,
    cumulative: u64,
    temp_writes: u64,
    temp_occupancy: u64,
    entries: u64,
    images: u64,
    temp_files: HashMap<std::path::PathBuf, u64>,
    entry_ids: HashMap<(std::path::PathBuf, usize), bool>,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self::new(ResourceLimits::PRODUCTION)
    }
}

impl ResourceBudget {
    pub(crate) fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            cumulative: 0,
            temp_writes: 0,
            temp_occupancy: 0,
            entries: 0,
            images: 0,
            temp_files: HashMap::new(),
            entry_ids: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn temp_counters(&self) -> (u64, u64) {
        (self.temp_writes, self.temp_occupancy)
    }

    pub(crate) fn entry(
        &mut self,
        archive: &std::path::Path,
        index: usize,
        image: bool,
    ) -> Result<(), AppError> {
        let key = (archive.to_path_buf(), index);
        let previous_image = self.entry_ids.get(&key).copied();
        let entries = self
            .entries
            .checked_add(u64::from(previous_image.is_none()))
            .ok_or(ResourceLimitKind::EntryCount)?;
        let images = self
            .images
            .checked_add(u64::from(image && previous_image != Some(true)))
            .ok_or(ResourceLimitKind::ImageEntryCount)?;
        if entries > self.limits.entries {
            return Err(ResourceLimitKind::EntryCount.into());
        }
        if images > self.limits.images {
            return Err(ResourceLimitKind::ImageEntryCount.into());
        }
        if previous_image.is_none() || (image && previous_image == Some(false)) {
            self.entry_ids
                .insert(key, image || previous_image == Some(true));
            self.entries = entries;
            self.images = images;
        }
        Ok(())
    }

    fn preflight_entry(&self, size: u64) -> Result<(), AppError> {
        if size > self.limits.single_entry {
            return Err(ResourceLimitKind::SingleEntryBytes.into());
        }
        Ok(())
    }

    fn consume(&mut self, amount: u64) -> Result<(), AppError> {
        let next = self
            .cumulative
            .checked_add(amount)
            .ok_or(ResourceLimitKind::CumulativeBytes)?;
        if next > self.limits.cumulative {
            return Err(ResourceLimitKind::CumulativeBytes.into());
        }
        self.cumulative = next;
        Ok(())
    }

    pub(crate) fn account_bytes(&mut self, amount: u64) -> Result<(), AppError> {
        self.preflight_entry(amount)?;
        self.consume(amount)
    }

    pub(crate) fn preflight(&self, amount: u64) -> Result<(), AppError> {
        self.preflight_entry(amount)
    }

    #[cfg(test)]
    pub(crate) fn account_temp_size(&mut self, amount: u64) -> Result<(), AppError> {
        let writes = self
            .temp_writes
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryWrites)?;
        let occupancy = self
            .temp_occupancy
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        if writes > self.limits.temp_writes {
            return Err(ResourceLimitKind::TemporaryWrites.into());
        }
        if occupancy > self.limits.temp_occupancy {
            return Err(ResourceLimitKind::TemporaryOccupancy.into());
        }
        self.temp_writes = writes;
        self.temp_occupancy = occupancy;
        Ok(())
    }

    pub(crate) fn write_temp(
        &mut self,
        path: &std::path::Path,
        bytes: &[u8],
    ) -> Result<(), AppError> {
        let amount = bytes.len() as u64;
        let writes = self
            .temp_writes
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryWrites)?;
        let previous = self.temp_files.get(path).copied().unwrap_or(0);
        let occupancy = self
            .temp_occupancy
            .checked_sub(previous)
            .and_then(|current| current.checked_add(amount))
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        if writes > self.limits.temp_writes {
            return Err(ResourceLimitKind::TemporaryWrites.into());
        }
        if occupancy > self.limits.temp_occupancy {
            return Err(ResourceLimitKind::TemporaryOccupancy.into());
        }
        self.temp_writes = writes;
        self.temp_occupancy = occupancy;
        self.temp_files.insert(path.to_path_buf(), amount);
        std::fs::write(path, bytes)?;
        Ok(())
    }

    fn begin_temp_file(&mut self, path: &std::path::Path) -> Result<(), AppError> {
        let previous = self.temp_files.get(path).copied().unwrap_or(0);
        self.temp_occupancy = self
            .temp_occupancy
            .checked_sub(previous)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        self.temp_files.insert(path.to_path_buf(), 0);
        Ok(())
    }

    fn account_temp_path_write(
        &mut self,
        path: &std::path::Path,
        amount: u64,
    ) -> Result<(), AppError> {
        let writes = self
            .temp_writes
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryWrites)?;
        let current = self.temp_files.get(path).copied().unwrap_or(0);
        let file_size = current
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        let occupancy = self
            .temp_occupancy
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        if writes > self.limits.temp_writes {
            return Err(ResourceLimitKind::TemporaryWrites.into());
        }
        if occupancy > self.limits.temp_occupancy {
            return Err(ResourceLimitKind::TemporaryOccupancy.into());
        }
        self.temp_writes = writes;
        self.temp_occupancy = occupancy;
        self.temp_files.insert(path.to_path_buf(), file_size);
        Ok(())
    }

    pub(crate) fn account_known_temp_path(
        &mut self,
        path: &std::path::Path,
        amount: u64,
    ) -> Result<(), AppError> {
        self.preflight_entry(amount)?;
        let cumulative = self
            .cumulative
            .checked_add(amount)
            .ok_or(ResourceLimitKind::CumulativeBytes)?;
        let writes = self
            .temp_writes
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryWrites)?;
        let previous = self.temp_files.get(path).copied().unwrap_or(0);
        let base_occupancy = self
            .temp_occupancy
            .checked_sub(previous)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        let occupancy = base_occupancy
            .checked_add(amount)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        if cumulative > self.limits.cumulative {
            return Err(ResourceLimitKind::CumulativeBytes.into());
        }
        if writes > self.limits.temp_writes {
            return Err(ResourceLimitKind::TemporaryWrites.into());
        }
        if occupancy > self.limits.temp_occupancy {
            return Err(ResourceLimitKind::TemporaryOccupancy.into());
        }
        self.cumulative = cumulative;
        self.temp_writes = writes;
        self.temp_occupancy = occupancy;
        self.temp_files.insert(path.to_path_buf(), amount);
        Ok(())
    }

    /// Releases occupancy after the tracked temporary tree has been removed.
    /// Cumulative writes deliberately remain unchanged; repeated release is safe.
    pub(crate) fn release_temp_tree(&mut self, root: &std::path::Path) -> Result<(), AppError> {
        let mut released = 0u64;
        for (path, size) in &self.temp_files {
            if path.starts_with(root) {
                released = released
                    .checked_add(*size)
                    .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
            }
        }
        let occupancy = self
            .temp_occupancy
            .checked_sub(released)
            .ok_or(ResourceLimitKind::TemporaryOccupancy)?;
        self.temp_files.retain(|path, _| !path.starts_with(root));
        self.temp_occupancy = occupancy;
        Ok(())
    }

    pub(crate) fn copy_to_path<R: Read + ?Sized>(
        &mut self,
        reader: &mut R,
        path: &std::path::Path,
        expected: Option<u64>,
    ) -> Result<u64, AppError> {
        if let Some(size) = expected {
            self.preflight_entry(size)?;
        }
        let mut output = std::fs::File::create(path)?;
        self.begin_temp_file(path)?;
        self.copy_to_writer(reader, path, &mut output)
    }

    fn copy_to_writer<R: Read + ?Sized, W: Write>(
        &mut self,
        reader: &mut R,
        path: &std::path::Path,
        output: &mut W,
    ) -> Result<u64, AppError> {
        let mut copied = 0u64;
        let mut chunk = [0; IO_CHUNK_SIZE];
        loop {
            let entry_left = self
                .limits
                .single_entry
                .checked_sub(copied)
                .ok_or(ResourceLimitKind::SingleEntryBytes)?;
            let cumulative_left = self
                .limits
                .cumulative
                .checked_sub(self.cumulative)
                .ok_or(ResourceLimitKind::CumulativeBytes)?;
            let allowance = entry_left.min(cumulative_left);
            let take = usize::try_from(allowance.saturating_add(1).min(chunk.len() as u64))
                .unwrap_or(chunk.len());
            let count = reader.read(&mut chunk[..take])?;
            if count == 0 {
                break;
            }
            let amount = count as u64;
            if amount > entry_left {
                return Err(ResourceLimitKind::SingleEntryBytes.into());
            }
            if amount > cumulative_left {
                return Err(ResourceLimitKind::CumulativeBytes.into());
            }
            self.consume(amount)?;
            self.account_temp_path_write(path, amount)?;
            output.write_all(&chunk[..count])?;
            copied = copied
                .checked_add(amount)
                .ok_or(ResourceLimitKind::SingleEntryBytes)?;
        }
        Ok(copied)
    }

    pub(crate) fn read_all<R: Read + ?Sized>(
        &mut self,
        reader: &mut R,
        expected: Option<u64>,
    ) -> Result<Vec<u8>, AppError> {
        if let Some(size) = expected {
            self.preflight_entry(size)?;
        }
        let mut data = Vec::new();
        let mut chunk = [0; IO_CHUNK_SIZE];
        let mut entry_bytes = 0u64;
        loop {
            let entry_left = self
                .limits
                .single_entry
                .checked_sub(entry_bytes)
                .ok_or(ResourceLimitKind::SingleEntryBytes)?;
            let cumulative_left = self
                .limits
                .cumulative
                .checked_sub(self.cumulative)
                .ok_or(ResourceLimitKind::CumulativeBytes)?;
            let allowance = entry_left.min(cumulative_left);
            let take = usize::try_from(allowance.saturating_add(1).min(chunk.len() as u64))
                .unwrap_or(chunk.len());
            let count = reader.read(&mut chunk[..take])?;
            if count == 0 {
                break;
            }
            let count = count as u64;
            let next = entry_bytes
                .checked_add(count)
                .ok_or(ResourceLimitKind::SingleEntryBytes)?;
            if count > entry_left {
                return Err(ResourceLimitKind::SingleEntryBytes.into());
            }
            if count > cumulative_left {
                return Err(ResourceLimitKind::CumulativeBytes.into());
            }
            self.consume(count)?;
            data.extend_from_slice(&chunk[..count as usize]);
            entry_bytes = next;
        }
        Ok(data)
    }

    pub(crate) fn drain<R: Read + ?Sized>(
        &mut self,
        reader: &mut R,
        expected: Option<u64>,
    ) -> Result<u64, AppError> {
        if let Some(size) = expected {
            self.preflight_entry(size)?;
        }
        let mut consumed = 0u64;
        let mut chunk = [0; IO_CHUNK_SIZE];
        loop {
            let entry_left = self
                .limits
                .single_entry
                .checked_sub(consumed)
                .ok_or(ResourceLimitKind::SingleEntryBytes)?;
            let cumulative_left = self
                .limits
                .cumulative
                .checked_sub(self.cumulative)
                .ok_or(ResourceLimitKind::CumulativeBytes)?;
            let allowed = entry_left.min(cumulative_left);
            let take = usize::try_from(allowed.saturating_add(1).min(chunk.len() as u64))
                .unwrap_or(chunk.len());
            let count = reader.read(&mut chunk[..take])?;
            if count == 0 {
                break;
            }
            let count = count as u64;
            if count > entry_left {
                return Err(ResourceLimitKind::SingleEntryBytes.into());
            }
            if count > cumulative_left {
                return Err(ResourceLimitKind::CumulativeBytes.into());
            }
            self.consume(count)?;
            consumed = consumed
                .checked_add(count)
                .ok_or(ResourceLimitKind::SingleEntryBytes)?;
        }
        Ok(consumed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn tiny() -> ResourceBudget {
        ResourceBudget::new(ResourceLimits {
            single_entry: 4,
            cumulative: 6,
            temp_writes: 6,
            temp_occupancy: 6,
            entries: 2,
            images: 1,
        })
    }

    #[test]
    fn limits_are_inclusive_and_overflow_fails_closed() {
        let mut budget = tiny();
        assert_eq!(
            budget.read_all(&mut &b"1234"[..], Some(4)).unwrap(),
            b"1234"
        );
        assert!(matches!(
            tiny().read_all(&mut &b"12345"[..], None),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::SingleEntryBytes
            ))
        ));
        let mut budget = tiny();
        budget.consume(6).unwrap();
        assert!(matches!(
            budget.consume(1),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::CumulativeBytes
            ))
        ));
        budget.cumulative = u64::MAX;
        assert!(matches!(
            budget.consume(1),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::CumulativeBytes
            ))
        ));
    }

    #[test]
    fn read_stops_at_the_first_byte_over_the_cumulative_limit() {
        let mut limits = ResourceLimits {
            single_entry: 8,
            cumulative: 5,
            temp_writes: 8,
            temp_occupancy: 8,
            entries: 1,
            images: 1,
        };
        let mut budget = ResourceBudget::new(limits);
        budget.account_bytes(4).unwrap();
        let mut input = std::io::Cursor::new(b"xyz");
        assert!(matches!(
            budget.read_all(&mut input, None),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::CumulativeBytes
            ))
        ));
        assert_eq!(input.position(), 2);

        limits.cumulative = 2;
        let mut budget = ResourceBudget::new(limits);
        let mut input = std::io::Cursor::new(b"123");
        assert!(matches!(
            budget.read_all(&mut input, None),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::CumulativeBytes
            ))
        ));
        assert_eq!(input.position(), 3);
    }

    #[test]
    fn counts_mixed_entries_with_distinct_resource_kinds() {
        let mut budget = tiny();
        budget
            .entry(std::path::Path::new("archive"), 0, true)
            .unwrap();
        budget
            .entry(std::path::Path::new("archive"), 1, false)
            .unwrap();
        assert!(matches!(
            budget.entry(std::path::Path::new("archive"), 2, false),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::EntryCount
            ))
        ));
        let mut budget = tiny();
        budget
            .entry(std::path::Path::new("archive"), 0, true)
            .unwrap();
        assert!(matches!(
            budget.entry(std::path::Path::new("archive"), 1, true),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::ImageEntryCount
            ))
        ));
    }

    #[test]
    fn rescanning_an_archive_does_not_count_the_same_entry_twice() {
        let mut budget = tiny();
        let archive = std::path::Path::new("archive.zip");
        budget.entry(archive, 0, true).unwrap();
        budget.entry(archive, 1, false).unwrap();
        budget.entry(archive, 0, true).unwrap();
        assert_eq!((budget.entries, budget.images), (2, 1));
    }

    #[test]
    fn temporary_write_and_occupancy_are_separate_bounded_counters() {
        let mut limits = ResourceLimits {
            single_entry: 8,
            cumulative: 8,
            temp_writes: 4,
            temp_occupancy: 3,
            entries: 1,
            images: 1,
        };
        let mut budget = ResourceBudget::new(limits);
        budget.account_temp_size(3).unwrap();
        assert!(matches!(
            budget.account_temp_size(1),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryOccupancy
            ))
        ));
        limits.temp_writes = 3;
        let mut budget = ResourceBudget::new(limits);
        budget.account_temp_size(3).unwrap();
        assert!(matches!(
            budget.account_temp_size(1),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryWrites
            ))
        ));
        budget.temp_writes = u64::MAX;
        assert!(matches!(
            budget.account_temp_size(1),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryWrites
            ))
        ));
    }

    #[test]
    fn overwriting_temp_path_replaces_occupancy_but_accumulates_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("entry");
        let limits = ResourceLimits {
            single_entry: 8,
            cumulative: 8,
            temp_writes: 6,
            temp_occupancy: 4,
            entries: 1,
            images: 1,
        };
        let mut budget = ResourceBudget::new(limits);
        budget.write_temp(&path, b"1234").unwrap();
        budget.write_temp(&path, b"56").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"56");
        assert_eq!(budget.temp_occupancy, 2);
        assert_eq!(budget.temp_writes, 6);
    }

    #[test]
    fn rejected_temp_overwrite_keeps_existing_live_data_accounted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("entry");
        let mut budget = ResourceBudget::new(ResourceLimits {
            single_entry: 8,
            cumulative: 8,
            temp_writes: 6,
            temp_occupancy: 4,
            entries: 1,
            images: 1,
        });
        budget.write_temp(&path, b"1234").unwrap();

        assert!(matches!(
            budget.write_temp(&path, b"567"),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryWrites
            ))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"1234");
        assert_eq!(budget.temp_counters(), (4, 4));
    }

    #[test]
    fn removed_temp_tree_releases_only_live_occupancy() {
        let root = tempfile::tempdir().unwrap();
        let a_dir = tempfile::tempdir_in(root.path()).unwrap();
        let a_path = a_dir.path().join("a");
        let b_path = root.path().join("b");
        let limits = ResourceLimits {
            single_entry: 8,
            cumulative: 8,
            temp_writes: 8,
            temp_occupancy: 4,
            entries: 1,
            images: 1,
        };
        let mut budget = ResourceBudget::new(limits);
        budget.write_temp(&a_path, b"1234").unwrap();
        let a_dir_path = a_dir.path().to_path_buf();
        drop(a_dir);
        budget.release_temp_tree(&a_dir_path).unwrap();
        budget.release_temp_tree(&a_dir_path).unwrap();
        budget.write_temp(&b_path, b"xy").unwrap();

        assert_eq!(budget.temp_writes, 6);
        assert_eq!(budget.temp_occupancy, 2);
        assert!(matches!(
            budget.write_temp(&root.path().join("c"), b"123"),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryWrites
            ))
        ));
        assert_eq!(budget.temp_occupancy, 2);
    }

    #[test]
    fn bounded_io_keeps_exact_limits_and_rejects_the_first_extra_byte() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("entry");
        for operation in ["read_all", "copy_to_path", "drain"] {
            for (input, expected, cumulative_used, error) in [
                (&b"1234"[..], Some(4), 0, None),
                (
                    &b"12345"[..],
                    Some(1),
                    0,
                    Some(ResourceLimitKind::SingleEntryBytes),
                ),
                (&b"1234"[..], None, 2, None),
                (
                    &b"12345"[..],
                    None,
                    2,
                    Some(ResourceLimitKind::CumulativeBytes),
                ),
            ] {
                let mut budget = ResourceBudget::new(ResourceLimits {
                    single_entry: if cumulative_used > 0 { 8 } else { 4 },
                    ..tiny().limits
                });
                budget.consume(cumulative_used).unwrap();
                let mut reader = std::io::Cursor::new(input);
                let result = match operation {
                    "read_all" => budget
                        .read_all(&mut reader, expected)
                        .map(|data| data.len() as u64),
                    "copy_to_path" => budget.copy_to_path(&mut reader, &path, expected),
                    _ => budget.drain(&mut reader, expected),
                };
                if let Some(kind) = error {
                    assert!(
                        matches!(result, Err(AppError::ArchiveResourceLimit(actual)) if actual == kind),
                        "{operation}"
                    );
                    assert_eq!(reader.position(), 5, "{operation}");
                } else {
                    assert_eq!(result.unwrap(), 4, "{operation}");
                }
            }
            let mut reader = std::io::Cursor::new(b"x");
            let mut budget = tiny();
            let result = match operation {
                "read_all" => budget
                    .read_all(&mut reader, Some(5))
                    .map(|data| data.len() as u64),
                "copy_to_path" => budget.copy_to_path(&mut reader, &path, Some(5)),
                _ => budget.drain(&mut reader, Some(5)),
            };
            assert!(matches!(
                result,
                Err(AppError::ArchiveResourceLimit(
                    ResourceLimitKind::SingleEntryBytes
                ))
            ));
            assert_eq!(reader.position(), 0, "{operation}");
        }
    }

    #[test]
    fn bounded_io_handles_short_reads_eof_and_io_errors() {
        struct ShortReader<'a>(&'a [u8]);
        impl Read for ShortReader<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let take = output.len().min(2);
                self.0.read(&mut output[..take])
            }
        }
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("entry");
        assert_eq!(
            tiny().read_all(&mut ShortReader(b"abc"), Some(1)).unwrap(),
            b"abc"
        );
        assert_eq!(tiny().drain(&mut ShortReader(b"abc"), None).unwrap(), 3);
        assert_eq!(
            tiny()
                .copy_to_path(&mut ShortReader(b"abc"), &path, None)
                .unwrap(),
            3
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        assert!(matches!(
            tiny().read_all(&mut FailingReader, None),
            Err(AppError::Io(_))
        ));
        assert!(matches!(
            tiny().drain(&mut FailingReader, None),
            Err(AppError::Io(_))
        ));
        assert!(matches!(
            tiny().copy_to_path(&mut FailingReader, &path, None),
            Err(AppError::Io(_))
        ));
        let mut budget = tiny();
        budget.begin_temp_file(&path).unwrap();
        assert!(matches!(
            budget.copy_to_writer(&mut &b"x"[..], &path, &mut FailingWriter),
            Err(AppError::Io(_))
        ));
    }

    #[test]
    fn entry_identity_upgrade_and_production_count_boundaries() {
        let mut budget = ResourceBudget::default();
        let a = std::path::Path::new("outer/a.zip");
        let b = std::path::Path::new("outer/b.zip");
        budget.entry(a, 0, false).unwrap();
        budget.entry(a, 0, false).unwrap();
        budget.entry(a, 0, true).unwrap();
        budget.entry(a, 0, true).unwrap();
        budget.entry(b, 0, true).unwrap();
        assert_eq!((budget.entries, budget.images), (2, 2));
        for index in 1..4095 {
            budget.entry(a, index, true).unwrap();
        }
        assert_eq!((budget.entries, budget.images), (4096, 4096));
        assert!(matches!(
            budget.entry(a, 4095, false),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::EntryCount
            ))
        ));
        budget.entry(a, 1, true).unwrap();
        let mut image_budget = ResourceBudget::new(ResourceLimits {
            entries: 4097,
            ..ResourceLimits::PRODUCTION
        });
        for index in 0..4096 {
            image_budget.entry(a, index, true).unwrap();
        }
        assert!(matches!(
            image_budget.entry(a, 4096, true),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::ImageEntryCount
            ))
        ));
    }

    #[test]
    fn temporary_path_limits_are_inclusive_and_release_only_occupancy() {
        let directory = tempfile::tempdir().unwrap();
        let a = directory.path().join("a");
        let b = directory.path().join("b");
        let mut budget = ResourceBudget::new(ResourceLimits {
            temp_writes: 6,
            temp_occupancy: 4,
            ..tiny().limits
        });
        budget.write_temp(&a, b"1234").unwrap();
        assert!(matches!(
            budget.write_temp(&b, b"x"),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryOccupancy
            ))
        ));
        budget.write_temp(&a, b"56").unwrap();
        assert_eq!(budget.temp_counters(), (6, 2));
        assert!(matches!(
            budget.write_temp(&b, b"x"),
            Err(AppError::ArchiveResourceLimit(
                ResourceLimitKind::TemporaryWrites
            ))
        ));
        budget.release_temp_tree(directory.path()).unwrap();
        assert_eq!(budget.temp_counters(), (6, 0));
    }

    #[test]
    fn copy_to_path_enforces_temp_limits_before_writing_the_extra_byte() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("entry");
        for (writes, occupancy, kind) in [
            (4, 8, ResourceLimitKind::TemporaryWrites),
            (8, 4, ResourceLimitKind::TemporaryOccupancy),
        ] {
            let limits = ResourceLimits {
                single_entry: 8,
                cumulative: 8,
                temp_writes: writes,
                temp_occupancy: occupancy,
                entries: 1,
                images: 1,
            };
            assert_eq!(
                ResourceBudget::new(limits)
                    .copy_to_path(&mut &b"1234"[..], &path, Some(4))
                    .unwrap(),
                4
            );
            let mut reader = std::io::Cursor::new(b"12345");
            assert!(matches!(
                ResourceBudget::new(limits).copy_to_path(&mut reader, &path, Some(1)),
                Err(AppError::ArchiveResourceLimit(actual)) if actual == kind
            ));
            assert_eq!(reader.position(), 5);
            assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        }
    }

    // Run with: cargo test archive::resource::tests::measure_resource_cost -- --ignored --nocapture
    #[test]
    #[ignore]
    fn measure_resource_cost() {
        struct Counted<'a> {
            bytes: &'a [u8],
            calls: usize,
        }
        impl Read for Counted<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                self.calls += 1;
                self.bytes.read(output)
            }
        }
        fn median(mut times: Vec<Duration>) -> Duration {
            times.sort();
            times[times.len() / 2]
        }
        let directory = tempfile::tempdir().unwrap();
        for size in [64 * 1024, 1024 * 1024, 16 * 1024 * 1024, 64 * 1024 * 1024] {
            let bytes = vec![7; size];
            for kind in ["read_all", "copy_to_path", "drain"] {
                let mut times = Vec::new();
                let mut calls = 0;
                for _ in 0..5 {
                    let mut budget = ResourceBudget::default();
                    let mut reader = Counted {
                        bytes: &bytes,
                        calls: 0,
                    };
                    let now = Instant::now();
                    match kind {
                        "read_all" => {
                            assert_eq!(
                                budget
                                    .read_all(&mut reader, Some(size as u64))
                                    .unwrap()
                                    .len(),
                                size
                            );
                        }
                        "copy_to_path" => {
                            assert_eq!(
                                budget
                                    .copy_to_path(
                                        &mut reader,
                                        &directory.path().join("output"),
                                        Some(size as u64)
                                    )
                                    .unwrap(),
                                size as u64
                            );
                        }
                        _ => assert_eq!(
                            budget.drain(&mut reader, Some(size as u64)).unwrap(),
                            size as u64
                        ),
                    }
                    times.push(now.elapsed());
                    calls = reader.calls;
                }
                eprintln!(
                    "resource kind={kind} size={size} chunk={IO_CHUNK_SIZE} median_us={} calls={calls}",
                    median(times).as_micros()
                );
            }
        }

        for count in [1, 100, 1000, 4096] {
            let archive = std::path::Path::new("long/path/to/a/nested/archive/book.cbz");
            let mut samples = Vec::new();
            let mut clone_samples = Vec::new();
            let mut lookup_samples = Vec::new();
            for _ in 0..5 {
                let mut budget = ResourceBudget::default();
                let now = Instant::now();
                for index in 0..count {
                    budget.entry(archive, index, false).unwrap();
                }
                let insert = now.elapsed();
                let now = Instant::now();
                for index in 0..count {
                    budget.entry(archive, index, false).unwrap();
                }
                let repeat = now.elapsed();
                let now = Instant::now();
                for index in 0..count {
                    budget.entry(archive, index, true).unwrap();
                }
                let upgrade = now.elapsed();
                samples.push((insert, repeat, upgrade));
                let now = Instant::now();
                let keys = (0..count)
                    .map(|index| (archive.to_path_buf(), index))
                    .collect::<Vec<_>>();
                clone_samples.push(now.elapsed());
                let now = Instant::now();
                for key in &keys {
                    std::hint::black_box(budget.entry_ids.get(key));
                }
                lookup_samples.push(now.elapsed());
            }
            let mut insert = samples.iter().map(|sample| sample.0).collect();
            let mut repeat = samples.iter().map(|sample| sample.1).collect();
            let mut upgrade = samples.iter().map(|sample| sample.2).collect();
            eprintln!(
                "entry count={count} insert_us={} repeat_us={} upgrade_us={} path_clone_us={} lookup_us={}",
                median(std::mem::take(&mut insert)).as_micros(),
                median(std::mem::take(&mut repeat)).as_micros(),
                median(std::mem::take(&mut upgrade)).as_micros(),
                median(clone_samples).as_micros(),
                median(lookup_samples).as_micros()
            );
        }

        for count in [1, 100, 1000, 4096] {
            let mut samples = Vec::new();
            let mut clone_samples = Vec::new();
            let mut map_samples = Vec::new();
            let mut accounting_samples = Vec::new();
            for _ in 0..5 {
                let mut budget = ResourceBudget::default();
                let tree = tempfile::tempdir_in(directory.path()).unwrap();
                let paths = (0..count)
                    .map(|index| tree.path().join(format!("entry-{index}")))
                    .collect::<Vec<_>>();
                let now = Instant::now();
                for path in &paths {
                    budget.write_temp(path, b"x").unwrap();
                }
                let insert = now.elapsed();
                let now = Instant::now();
                for path in &paths {
                    budget.write_temp(path, b"x").unwrap();
                }
                let update = now.elapsed();
                let now = Instant::now();
                let cloned = paths
                    .iter()
                    .map(|path| path.to_path_buf())
                    .collect::<Vec<_>>();
                clone_samples.push(now.elapsed());
                let now = Instant::now();
                for path in cloned {
                    budget.temp_files.insert(path, 1);
                }
                map_samples.push(now.elapsed());
                for path in &paths {
                    budget.begin_temp_file(path).unwrap();
                }
                let now = Instant::now();
                for path in &paths {
                    budget.account_temp_path_write(path, 1).unwrap();
                }
                accounting_samples.push(now.elapsed());
                drop(tree);
                let now = Instant::now();
                budget.release_temp_tree(directory.path()).unwrap();
                let release = now.elapsed();
                samples.push((insert, update, release));
            }
            eprintln!(
                "temp count={count} insert_us={} update_us={} release_us={} path_clone_us={} map_update_us={} account_us={}",
                median(samples.iter().map(|sample| sample.0).collect()).as_micros(),
                median(samples.iter().map(|sample| sample.1).collect()).as_micros(),
                median(samples.iter().map(|sample| sample.2).collect()).as_micros(),
                median(clone_samples).as_micros(),
                median(map_samples).as_micros(),
                median(accounting_samples).as_micros()
            );
        }
    }
}
