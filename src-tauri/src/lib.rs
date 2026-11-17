use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

// ============ ПРАВИЛА: расширение -> папка ============
// Хочешь добавить тип файла? Допиши его сюда.
fn category_for(ext: &str) -> Option<&'static str> {
    match ext {
        "pdf" | "doc" | "docx" | "txt" | "odt" | "rtf" | "xls" | "xlsx" | "ppt" | "pptx"
        | "csv" => Some("Documents"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "heic" => Some("Images"),
        "zip" | "rar" | "7z" | "tar" | "gz" => Some("Archives"),
        "exe" | "msi" => Some("Installers"),
        "mp4" | "mkv" | "avi" | "mov" => Some("Video"),
        "mp3" | "wav" | "flac" | "ogg" => Some("Music"),
        _ => None, // всё незнакомое остаётся на месте
    }
}

// ============ ТИПЫ ДАННЫХ ============
#[derive(Serialize, Clone)]
struct Candidate {
    path: String,
    name: String,
    category: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct MoveRecord {
    from: String,
    to: String,
    time: u64,
}

// ============ ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ ============
fn desktop() -> Result<PathBuf, String> {
    dirs::desktop_dir().ok_or_else(|| "Не найдена папка Desktop".to_string())
}

fn sorted_root() -> Result<PathBuf, String> {
    let docs = dirs::document_dir().ok_or_else(|| "Не найдена папка Documents".to_string())?;
    Ok(docs.join("PetSorted"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// Подходит ли файл под правила? Если да, возвращаем описание.
fn to_candidate(path: &Path) -> Option<Candidate> {
    if !path.is_file() {
        return None;
    }
    let name = path.file_name()?.to_str()?.to_string();
    if name.starts_with('.') {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_lowercase();
    let category = category_for(&ext)?;

    // Совсем свежие файлы пропускаем: возможно, они ещё скачиваются
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::from_secs(3600));
    if age < Duration::from_secs(3) {
        return None;
    }

    Some(Candidate {
        path: path.to_string_lossy().to_string(),
        name,
        category: category.to_string(),
    })
}

// Если файл с таким именем уже есть, добавляем " (1)", " (2)" и т.д.
fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut n = 1;
    loop {
        let candidate = dir.join(format!("{} ({}){}", stem, n, ext));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

// Перемещение: сначала быстрый rename, при неудаче (другой диск) копирование + удаление
fn move_any(from: &Path, to: &Path) -> Result<(), String> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).map_err(|e| format!("Не удалось скопировать: {e}"))?;
    if let Err(e) = fs::remove_file(from) {
        let _ = fs::remove_file(to); // откатываем копию, чтобы не было дубля
        return Err(format!("Не удалось убрать оригинал: {e}"));
    }
    Ok(())
}

// ============ ЖУРНАЛ ============
fn journal_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("journal.json"))
}

fn read_journal(app: &AppHandle) -> Vec<MoveRecord> {
    let Ok(path) = journal_path(app) else {
        return Vec::new();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_journal(app: &AppHandle, journal: &Vec<MoveRecord>) -> Result<(), String> {
    let path = journal_path(app)?;
    let text = serde_json::to_string_pretty(journal).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

// ============ КОМАНДЫ (их вызывает JS) ============

// Какие файлы на рабочем столе можно убрать?
#[tauri::command]
fn scan_desktop() -> Result<Vec<Candidate>, String> {
    let desktop = desktop()?;
    let mut result = Vec::new();
    for entry in fs::read_dir(&desktop).map_err(|e| e.to_string())? {
        let Ok(entry) = entry else { continue };
        if let Some(candidate) = to_candidate(&entry.path()) {
            result.push(candidate);
        }
    }
    Ok(result)
}

// Переместить один файл по правилам
#[tauri::command]
fn move_file(app: AppHandle, path: String, dry_run: bool) -> Result<MoveRecord, String> {
    let src = PathBuf::from(&path);

    // Защита: двигаем только файлы, лежащие прямо на рабочем столе
    if src.parent() != Some(desktop()?.as_path()) {
        return Err("Файл не на рабочем столе".to_string());
    }
    let candidate = to_candidate(&src).ok_or("Файл не подходит под правила")?;

    let dest_dir = sorted_root()?.join(&candidate.category);
    let dest = unique_path(&dest_dir.join(&candidate.name));

    let record = MoveRecord {
        from: path,
        to: dest.to_string_lossy().to_string(),
        time: now_secs(),
    };

    if dry_run {
        return Ok(record); // ничего не трогаем
    }

    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    move_any(&src, &dest)?;

    let mut journal = read_journal(&app);
    journal.push(record.clone());
    write_journal(&app, &journal)?;

    Ok(record)
}

// Отменить последнее перемещение. Возвращает путь, куда файл вернулся.
#[tauri::command]
fn undo_last(app: AppHandle) -> Result<Option<String>, String> {
    let mut journal = read_journal(&app);
    let Some(record) = journal.pop() else {
        return Ok(None); // журнал пуст
    };

    let current = PathBuf::from(&record.to);
    if !current.exists() {
        write_journal(&app, &journal)?;
        return Err("Файла уже нет на месте (его переместили или удалили)".to_string());
    }

    let back = unique_path(&PathBuf::from(&record.from));
    move_any(&current, &back)?;
    write_journal(&app, &journal)?;

    Ok(Some(back.to_string_lossy().to_string()))
}

// ============ СЛЕЖЕНИЕ ЗА РАБОЧИМ СТОЛОМ ============
fn start_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let Some(desktop) = dirs::desktop_dir() else {
            eprintln!("Не найдена папка Desktop");
            return;
        };

        let mut watcher = match notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = app.emit("desktop-changed", ());
                }
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Не удалось создать наблюдатель: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&desktop, RecursiveMode::NonRecursive) {
            eprintln!("Не удалось следить за папкой: {e}");
            return;
        }

        // watcher работает, только пока он жив, поэтому держим поток
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    });
}

// ============ ЗАПУСК ============
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            start_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![scan_desktop, move_file, undo_last])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}