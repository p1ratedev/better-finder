//! The text of the list columns, like Finder.
//!
//! - Size: `NSByteCountFormatter` (file style), so the text follows the
//!   locale (`2,5 MB` in Brazil). Folders and packages show `--`.
//! - Kind: the Launch Services kind of the extension, like Finder
//!   (`ZIP archive`, `Plain Text`). The loader thread gets it for one file of
//!   each new extension (`prefetch_kinds`) and caches it by extension. The
//!   draw code uses that cache, else the `UTType` description.
//! - Dates: `date-format = finder` uses one cached `NSDateFormatter` (medium
//!   date, short time, relative). Other formats use `bf_core::format`.
//!
//! All functions run on the main thread (the draw code). They do no disk
//! access: the type information comes from the extension only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use bf_core::config::Column;
use bf_core::format::format_time;
use bf_core::{Entry, EntryKind};
use objc2::rc::Retained;
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSByteCountFormatterUnits, NSDate,
    NSDateFormatter, NSDateFormatterStyle, NSString,
};
use objc2_uniform_type_identifiers::{UTType, UTTypeFolder, UTTypeUnixExecutable};

/// The title of a column in the column header.
pub fn title(column: Column) -> &'static str {
    match column {
        Column::Size => "Size",
        Column::Kind => "Kind",
        Column::Modified => "Date Modified",
        Column::Created => "Date Created",
        Column::Extension => "Extension",
    }
}

struct Formatters {
    bytes: Retained<NSByteCountFormatter>,
    date: Retained<NSDateFormatter>,
    /// Lowercase extension to the kind text.
    kinds: HashMap<String, String>,
    folder: String,
    alias: String,
    executable: String,
}

thread_local! {
    static FORMATTERS: RefCell<Option<Formatters>> = const { RefCell::new(None) };
}

fn description(kind: &UTType) -> Option<String> {
    kind.localizedDescription().map(|d| d.to_string())
}

/// Finder writes `Folder`; the type description is `folder`.
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Lowercase extension to the Launch Services kind. Threads share it.
fn kind_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The most new extensions that one load asks Launch Services for.
const PREFETCH_LIMIT: usize = 64;

/// Gets the Launch Services kind of each new extension in `entries`, from
/// one file of that extension. It accesses the disk: call it on a
/// background thread (the loader does).
pub fn prefetch_kinds(entries: &[Entry]) {
    use objc2_foundation::{NSArray, NSURL, NSURLLocalizedTypeDescriptionKey};
    let mut wanted: Vec<(String, &Entry)> = Vec::new();
    {
        let cache = kind_cache().lock().unwrap_or_else(PoisonError::into_inner);
        for entry in entries {
            if entry.kind == EntryKind::Symlink
                || (entry.kind == EntryKind::Directory && !entry.is_package)
            {
                continue;
            }
            let ext = entry.extension().to_lowercase();
            if ext.is_empty() || cache.contains_key(&ext) || wanted.iter().any(|(e, _)| *e == ext) {
                continue;
            }
            wanted.push((ext, entry));
            if wanted.len() >= PREFETCH_LIMIT {
                break;
            }
        }
    }
    if wanted.is_empty() {
        return;
    }
    // SAFETY: The resource key is an immutable static from Foundation.
    let key = unsafe { NSURLLocalizedTypeDescriptionKey };
    let keys = NSArray::from_slice(&[key]);
    let found: Vec<(String, String)> = wanted
        .into_iter()
        .filter_map(|(ext, entry)| {
            let url = NSURL::from_file_path(&entry.path)?;
            let values = url.resourceValuesForKeys_error(&keys).ok()?;
            let kind = values.objectForKey(key)?.downcast::<NSString>().ok()?;
            Some((ext, kind.to_string()))
        })
        .collect();
    let mut cache = kind_cache().lock().unwrap_or_else(PoisonError::into_inner);
    cache.extend(found);
}

impl Formatters {
    fn new() -> Formatters {
        let bytes = NSByteCountFormatter::new();
        bytes.setCountStyle(NSByteCountFormatterCountStyle::File);
        // All units, like Finder: `Zero bytes` and `7 bytes`, not `Zero KB`.
        bytes.setAllowedUnits(NSByteCountFormatterUnits::UseAll);
        let date = NSDateFormatter::new();
        date.setDateStyle(NSDateFormatterStyle::MediumStyle);
        date.setTimeStyle(NSDateFormatterStyle::ShortStyle);
        date.setDoesRelativeDateFormatting(true);
        // SAFETY: The type constants are immutable statics.
        let (folder, executable) = unsafe { (UTTypeFolder, UTTypeUnixExecutable) };
        Formatters {
            bytes,
            date,
            kinds: HashMap::new(),
            folder: description(folder)
                .map(|d| capitalize_first(&d))
                .unwrap_or_else(|| "Folder".to_string()),
            // Finder calls a symlink an alias.
            alias: "Alias".to_string(),
            executable: description(executable)
                .unwrap_or_else(|| "Unix Executable File".to_string()),
        }
    }

    fn kind(&mut self, entry: &Entry) -> String {
        if entry.kind == EntryKind::Symlink {
            return self.alias.clone();
        }
        if entry.kind == EntryKind::Directory && !entry.is_package {
            return self.folder.clone();
        }
        let extension = entry.extension().to_lowercase();
        if extension.is_empty() {
            return if entry.executable {
                self.executable.clone()
            } else {
                "Document".to_string()
            };
        }
        let shared = kind_cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&extension)
            .cloned();
        if let Some(kind) = shared {
            return kind;
        }
        self.kinds
            .entry(extension)
            .or_insert_with_key(|ext| {
                UTType::typeWithFilenameExtension(&NSString::from_str(ext))
                    .filter(|t| !t.isDynamic())
                    .and_then(|t| description(&t))
                    .unwrap_or_else(|| "Document".to_string())
            })
            .clone()
    }
}

fn with_formatters<R>(f: impl FnOnce(&mut Formatters) -> R) -> Option<R> {
    FORMATTERS.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        Some(f(guard.get_or_insert_with(Formatters::new)))
    })
}

/// Forgets the cached formatters, so that a change of the locale applies.
pub fn reset() {
    FORMATTERS.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            *guard = None;
        }
    });
}

/// A byte count in the file style of the locale, for example `123,4 GB`.
pub fn byte_count(bytes: u64) -> String {
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    with_formatters(|f| f.bytes.stringFromByteCount(bytes).to_string())
        .unwrap_or_else(|| bf_core::format::human_size(bytes.unsigned_abs()))
}

/// The text of one column for `entry`.
pub fn cell_text(column: Column, entry: &Entry, date_format: &str, now: SystemTime) -> String {
    match column {
        Column::Size => {
            if entry.is_dir || entry.is_package || entry.kind == EntryKind::Directory {
                "--".to_string()
            } else {
                byte_count(entry.size)
            }
        }
        Column::Kind => with_formatters(|f| f.kind(entry)).unwrap_or_default(),
        Column::Modified => date_text(entry.modified, date_format, now),
        Column::Created => date_text(entry.created, date_format, now),
        Column::Extension => {
            if entry.kind == EntryKind::Directory && !entry.is_package {
                String::new()
            } else {
                entry.extension().to_string()
            }
        }
    }
}

fn date_text(time: Option<SystemTime>, date_format: &str, now: SystemTime) -> String {
    let Some(time) = time else {
        return "--".to_string();
    };
    if date_format != "finder" {
        return format_time(time, date_format, now);
    }
    let secs = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    };
    let date = NSDate::dateWithTimeIntervalSince1970(secs);
    with_formatters(|f| f.date.stringFromDate(&date).to_string())
        .unwrap_or_else(|| format_time(time, "finder", now))
}

/// A wide sample date (December 28, 23:58) in `date_format`, to measure the
/// width of the date columns.
pub fn sample_date_text(date_format: &str) -> String {
    use chrono::{Local, TimeZone};
    let sample = Local
        .with_ymd_and_hms(2026, 12, 28, 23, 58, 58)
        .single()
        .map(SystemTime::from)
        .unwrap_or_else(SystemTime::now);
    if date_format == "relative" {
        return "Yesterday 23:59".to_string();
    }
    format_time(sample, date_format, sample)
}
