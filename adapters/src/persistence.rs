//! スナップショット原子永続化およびトレースログ読み書き（EK-DS-001 1.3 / 3.1 / 3.2 / 4.3節）。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use ek_core::{
    ERR_INVALID_ARGUMENT, ERR_PORT_FAILURE, ERR_SNAPSHOT_INVALID, ERR_SNAPSHOT_IO, PortError,
    SnapshotPort, TraceHeader, TraceRecord, TraceSink, validate_snapshot_format,
};

/// `run_id` の制約（EK-DS-001 1.3節: 非空かつ英数字と `-` のみ、`_` 禁止）を検証する。
pub fn validate_run_id(run_id: &str) -> Result<(), i32> {
    if run_id.is_empty()
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(ERR_INVALID_ARGUMENT);
    }
    Ok(())
}

/// スナップショット識別キー（EK-DD-001 2.8節）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotKey {
    pub run_id: String,
    pub logical_time_ns: i64,
    pub event_count: u64,
}

impl SnapshotKey {
    /// EK-DS-001 1.3節の正式ファイル名 `<run_id>_<logical_time_ns>_<event_count>.eksnap` を返す。
    pub fn file_name(&self) -> String {
        format!(
            "{}_{}_{}.eksnap",
            self.run_id, self.logical_time_ns, self.event_count
        )
    }

    /// EK-DS-001 3.1節の一時ファイル名 `.<run_id>_<logical_time_ns>_<event_count>.eksnap.tmp` を返す。
    pub fn tmp_file_name(&self) -> String {
        format!(
            ".{}_{}_{}.eksnap.tmp",
            self.run_id, self.logical_time_ns, self.event_count
        )
    }
}

/// インメモリ `SnapshotPort` 実装（DS 3.1節: バイト列構築後に差し替え、同名重複拒否）。
#[derive(Debug)]
pub struct MemorySnapshotStore {
    run_id: String,
    blobs: BTreeMap<(i64, u64), Vec<u8>>,
}

impl MemorySnapshotStore {
    /// `run_id` を検証してストアを作成する。
    pub fn new(run_id: impl Into<String>) -> Result<Self, i32> {
        let run_id = run_id.into();
        validate_run_id(&run_id)?;
        Ok(Self {
            run_id,
            blobs: BTreeMap::new(),
        })
    }

    /// 保持しているスナップショットを DS 4.3節形式検査つきで取得する。
    pub fn load(&self, logical_time_ns: i64, event_count: u64) -> Result<Vec<u8>, i32> {
        let blob = self
            .blobs
            .get(&(logical_time_ns, event_count))
            .ok_or(ERR_SNAPSHOT_IO)?;
        validate_snapshot_format(blob)?;
        Ok(blob.clone())
    }

    /// 保存済みキー一覧を返す。
    pub fn keys(&self) -> Vec<SnapshotKey> {
        self.blobs
            .keys()
            .map(|&(logical_time_ns, event_count)| SnapshotKey {
                run_id: self.run_id.clone(),
                logical_time_ns,
                event_count,
            })
            .collect()
    }
}

impl SnapshotPort for MemorySnapshotStore {
    fn store(
        &mut self,
        blob: &[u8],
        logical_time_ns: i64,
        event_count: u64,
    ) -> Result<(), PortError> {
        if self.blobs.contains_key(&(logical_time_ns, event_count)) {
            return Err(PortError(ERR_SNAPSHOT_IO));
        }
        let owned = blob.to_vec();
        self.blobs.insert((logical_time_ns, event_count), owned);
        Ok(())
    }
}

/// ディスク永続化 `SnapshotPort`（EK-DS-001 3.1節 原子書き込み契約準拠）。
#[derive(Clone, Debug)]
pub struct DiskSnapshotStore {
    dir: PathBuf,
    run_id: String,
    cleanup_count_on_open: usize,
}

impl DiskSnapshotStore {
    /// ディレクトリを作成（必要時）し、自 `run_id` の残存 `.eksnap.tmp` のみを清掃して開く。
    ///
    /// DS 3.1節手順5: 名前が `.<run_id>_` で始まり `.eksnap.tmp` で終わるファイルだけを削除する。
    pub fn open(dir: impl AsRef<Path>, run_id: impl Into<String>) -> Result<Self, i32> {
        let run_id = run_id.into();
        validate_run_id(&run_id)?;
        let dir_path = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir_path).map_err(|_| ERR_SNAPSHOT_IO)?;

        let tmp_prefix = format!(".{run_id}_");
        let tmp_suffix = ".eksnap.tmp";
        let mut cleaned = 0usize;
        let entries = fs::read_dir(&dir_path).map_err(|_| ERR_SNAPSHOT_IO)?;
        for entry in entries {
            let entry = entry.map_err(|_| ERR_SNAPSHOT_IO)?;
            let file_type = entry.file_type().map_err(|_| ERR_SNAPSHOT_IO)?;
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with(&tmp_prefix) && name.ends_with(tmp_suffix) {
                fs::remove_file(entry.path()).map_err(|_| ERR_SNAPSHOT_IO)?;
                cleaned += 1;
            }
        }

        Ok(Self {
            dir: dir_path,
            run_id,
            cleanup_count_on_open: cleaned,
        })
    }

    /// `open` 時に削除された自分の古い一時ファイル数を返す。
    pub fn cleaned_tmp_files(&self) -> usize {
        self.cleanup_count_on_open
    }

    /// 正式ファイルパスを返す。
    pub fn snapshot_path(&self, logical_time_ns: i64, event_count: u64) -> PathBuf {
        let key = SnapshotKey {
            run_id: self.run_id.clone(),
            logical_time_ns,
            event_count,
        };
        self.dir.join(key.file_name())
    }

    /// 保存済みスナップショットを読み込み、DS 4.3節の形式検査を行う。
    pub fn load(&self, logical_time_ns: i64, event_count: u64) -> Result<Vec<u8>, i32> {
        let path = self.snapshot_path(logical_time_ns, event_count);
        Self::load_from_path(&path)
    }

    /// 指定パスの `.eksnap` を読み込み、DS 4.3節の形式検査を行う。
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Vec<u8>, i32> {
        let bytes = fs::read(path).map_err(|_| ERR_SNAPSHOT_IO)?;
        validate_snapshot_format(&bytes)?;
        Ok(bytes)
    }

    fn sync_parent_dir(&self) -> Result<(), i32> {
        #[cfg(unix)]
        {
            let dir_file = File::open(&self.dir).map_err(|_| ERR_SNAPSHOT_IO)?;
            dir_file.sync_all().map_err(|_| ERR_SNAPSHOT_IO)?;
        }
        Ok(())
    }
}

impl SnapshotPort for DiskSnapshotStore {
    fn store(
        &mut self,
        blob: &[u8],
        logical_time_ns: i64,
        event_count: u64,
    ) -> Result<(), PortError> {
        let key = SnapshotKey {
            run_id: self.run_id.clone(),
            logical_time_ns,
            event_count,
        };
        let final_path = self.dir.join(key.file_name());
        let tmp_path = self.dir.join(key.tmp_file_name());

        // 既に正式ファイルが存在する場合は上書き禁止のため即座に失敗する（DS 3.1節・3.2節）。
        if final_path.exists() {
            return Err(PortError(ERR_SNAPSHOT_IO));
        }

        // 手順1: 排他的新規作成（create_new）で一時ファイルを生成し全量を書き出す。
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|_| PortError(ERR_SNAPSHOT_IO))?;

        let write_sync = (|| -> std::io::Result<()> {
            tmp_file.write_all(blob)?;
            tmp_file.sync_all()?;
            Ok(())
        })();
        drop(tmp_file);

        if write_sync.is_err() {
            let _ = fs::remove_file(&tmp_path);
            return Err(PortError(ERR_SNAPSHOT_IO));
        }

        // 手順3: 既存ファイルを上書きしない原子的公開（hard_link は既存先がある場合に必ず失敗する）。
        if fs::hard_link(&tmp_path, &final_path).is_err() {
            let _ = fs::remove_file(&tmp_path);
            return Err(PortError(ERR_SNAPSHOT_IO));
        }
        // hard_link 成功時点で最終スナップショットは公開済みのため、一時ファイル削除失敗では Err にしない
        // （次回起動時の `.tmp` 清掃で回収される）。
        let _ = fs::remove_file(&tmp_path);

        // 手順4: ディレクトリ同期。
        self.sync_parent_dir().map_err(PortError)?;
        Ok(())
    }
}

/// 追記専用トレースシンク（EK-DS-001 1.3 / 3.1節）。
#[derive(Debug)]
pub struct DiskTraceSink {
    path: PathBuf,
    file: File,
}

impl DiskTraceSink {
    /// `<dir>/<run_id>.ektrace` を排他新規作成する（既存ファイルがあれば失敗）。
    pub fn create(dir: impl AsRef<Path>, run_id: impl AsRef<str>) -> Result<Self, i32> {
        validate_run_id(run_id.as_ref())?;
        fs::create_dir_all(dir.as_ref()).map_err(|_| ERR_PORT_FAILURE)?;
        let path = dir.as_ref().join(format!("{}.ektrace", run_id.as_ref()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| ERR_PORT_FAILURE)?;
        Ok(Self { path, file })
    }

    /// 出力ファイルパスを返す。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// OS へフラッシュ・同期する。
    pub fn sync(&mut self) -> Result<(), i32> {
        self.file.flush().map_err(|_| ERR_PORT_FAILURE)?;
        self.file.sync_data().map_err(|_| ERR_PORT_FAILURE)?;
        Ok(())
    }
}

impl TraceSink for DiskTraceSink {
    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        self.file
            .write_all(&header.to_le_bytes())
            .and_then(|()| self.file.flush())
            .map_err(|_| PortError(ERR_PORT_FAILURE))
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        self.file
            .write_all(&record.to_le_bytes())
            .map_err(|_| PortError(ERR_PORT_FAILURE))
    }
}

/// トレースログ読み込み結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceLog {
    pub header: TraceHeader,
    pub records: Vec<TraceRecord>,
    pub truncated_tail_bytes: usize,
}

/// トレースログリーダー（EK-DS-001 3.1 / 4.3節）。
pub struct TraceLogReader;

impl TraceLogReader {
    /// トレースファイルを開いて検証・復号する（末尾端数レコードは破棄する）。
    pub fn read_file(path: impl AsRef<Path>) -> Result<TraceLog, i32> {
        let bytes = fs::read(path).map_err(|_| ERR_SNAPSHOT_IO)?;
        Self::from_bytes(&bytes)
    }

    /// バイト列から `TRACE_HEADER` を検査し、`record_size` 単位でレコードを読み込む。
    pub fn from_bytes(bytes: &[u8]) -> Result<TraceLog, i32> {
        let header = TraceHeader::from_le_bytes_validated(bytes).ok_or(ERR_SNAPSHOT_INVALID)?;
        let rec_size = header.record_size as usize;
        let body = &bytes[TraceHeader::SIZE..];
        let complete_records = body.len() / rec_size;
        let truncated_tail_bytes = body.len() % rec_size;

        let mut records = Vec::with_capacity(complete_records);
        for chunk in body[..complete_records * rec_size].chunks_exact(rec_size) {
            let rec = TraceRecord::from_le_bytes(chunk).ok_or(ERR_SNAPSHOT_INVALID)?;
            records.push(rec);
        }

        Ok(TraceLog {
            header,
            records,
            truncated_tail_bytes,
        })
    }
}
