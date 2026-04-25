use std::{
    io,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::types::Type;
use rusqlite::{Connection, Result, params};

use crate::common::{
    data::{IndexStatus, IndexType, Project},
    utils::hash_str,
};

/// SQLite 索引元数据：一个 `index.db` 只对应一个工程；不在此结构体上缓存 [`Project`]。
pub struct IndexStatusStore {
    db_path: PathBuf,
}

impl IndexStatusStore {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub fn open(&self) -> Result<Connection> {
        let conn = Connection::open(self.db_path.as_path())?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(conn)
    }

    fn get_project(&self, root_path: &Path, embedding_model: &str) -> Result<Option<Project>> {
        let root_canon = root_path
            .canonicalize()
            .map_err(|e| rusqlite::Error::InvalidPath(root_path.to_path_buf()))?;
        let root_str = root_canon.to_string_lossy();

        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT root_path, embedding_model, hash, index_finished_time FROM projects 
            WHERE root_path = ?1 AND embedding_model = ?2",
        )?;

        let mut rows = stmt.query(params![root_str.as_ref(), embedding_model])?;

        if let Some(row) = rows.next()? {
            Ok(Some(Project {
                root_path: PathBuf::from(row.get::<_, String>(0)?),
                embedding_model: row.get(1)?,
                hash: row.get(2)?,
                index_finished_time: Some(row.get(3)?),
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_or_create_project(
        &self,
        root_path: &Path,
        embedding_model: &str,
    ) -> Result<Project> {
        if let Some(project) = self.get_project(root_path, embedding_model)? {
            return Ok(project);
        }

        let root_canon = root_path
            .canonicalize()
            .map_err(|e| rusqlite::Error::InvalidPath(root_path.to_path_buf()))?;
        let root_str = root_canon.to_string_lossy();

        let conn = self.open()?;

        // One project row per database file.
        let mut stmt = conn.prepare("SELECT root_path, embedding_model FROM projects LIMIT 1")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let existing_root: String = row.get(0)?;
            let existing_model: String = row.get(1)?;
            if existing_root != root_str.as_ref() || existing_model != embedding_model {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
                    Some("this index database is already bound to another project".into()),
                ));
            }
        }

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();

        let hash = hash_str(root_str.as_ref());
        let mut stmt = conn.prepare(
            "INSERT INTO projects (root_path, embedding_model, hash, index_finished_time) 
            VALUES (?1, ?2, ?3, ?4)",
        )?;
        stmt.execute(params![
            root_str.as_ref(),
            embedding_model,
            hash,
            created_at
        ])?;

        let project = Project {
            root_path: root_path.to_path_buf(),
            embedding_model: embedding_model.to_string(),
            hash,
            index_finished_time: None,
        };
        Ok(project)
    }

    pub fn delete_project(&self, root_path: &Path, embedding_model: &str) -> Result<()> {
        let root_canon = root_path
            .canonicalize()
            .map_err(|e| rusqlite::Error::InvalidPath(root_path.to_path_buf()))?;
        let root_str = root_canon.to_string_lossy();

        let conn = self.open()?;
        conn.execute("DELETE FROM index_status", [])?;
        let mut stmt =
            conn.prepare("DELETE FROM projects WHERE root_path = ?1 AND embedding_model = ?2")?;
        stmt.execute(params![root_str.as_ref(), embedding_model])?;
        Ok(())
    }

    /// 测试用：清空本库内 `index_status` 与 `projects`。
    fn delete_all_projects(&self) -> Result<()> {
        let conn = self.open()?;
        conn.execute("DELETE FROM index_status", [])?;
        let mut stmt = conn.prepare("DELETE FROM projects")?;
        stmt.execute(params![])?;
        Ok(())
    }

    /// 单库至多一行工程元数据：更新 `index_finished_time`。
    pub fn update_project_index_finished_time(&self, index_finished_time: u64) -> Result<()> {
        let conn = self.open()?;
        let mut stmt = conn.prepare("UPDATE projects SET index_finished_time = ?1")?;
        stmt.execute(params![index_finished_time])?;
        Ok(())
    }

    /// 读取本库 `projects` 首行的完成时间（无行则 `None`）。
    pub fn get_project_index_finished_time(&self) -> Result<Option<u64>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare("SELECT index_finished_time FROM projects LIMIT 1")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_index_status_by_layer(&self, layer: IndexType) -> Result<Vec<IndexStatus>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT file_path, layer, file_hash, mtime, ctime, size, indexed_at 
            FROM index_status 
            WHERE layer = ?1",
        )?;
        let mut rows = stmt.query_map(params![layer.to_string()], |row| {
            Ok(IndexStatus {
                file_path: row.get(0)?,
                layer: IndexType::from_str(row.get::<_, String>(1)?.as_str()).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        Type::Text,
                        Box::new(io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
                    )
                })?,
                file_hash: row.get(2)?,
                mtime: row.get(3)?,
                ctime: row.get(4)?,
                size: row.get(5)?,
                indexed_at: row.get(6)?,
            })
        })?;

        let mut index_statuses = Vec::new();
        for row in rows {
            index_statuses.push(row?);
        }
        Ok(index_statuses)
    }

    pub fn get_index_status_by_path(
        &self,
        file_path: &str,
        layer: IndexType,
    ) -> Result<Option<IndexStatus>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT file_path, layer, file_hash, mtime, ctime, size, indexed_at 
            FROM index_status 
            WHERE file_path = ?1 AND layer = ?2",
        )?;
        let mut rows = stmt.query(params![file_path, layer.to_string()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(IndexStatus {
                file_path: row.get(0)?,
                layer: IndexType::from_str(row.get::<_, String>(1)?.as_str()).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        Type::Text,
                        Box::new(io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
                    )
                })?,
                file_hash: row.get(2)?,
                mtime: row.get(3)?,
                ctime: row.get(4)?,
                size: row.get(5)?,
                indexed_at: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn upsert_index_status(&self, index_status: &IndexStatus) -> Result<()> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "INSERT INTO index_status (file_path, layer, file_hash, mtime, ctime, size, indexed_at) 
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) 
            ON CONFLICT (file_path, layer) 
            DO UPDATE SET 
                file_hash = excluded.file_hash, 
                mtime = excluded.mtime, 
                ctime = excluded.ctime, 
                size = excluded.size, 
                indexed_at = excluded.indexed_at"
        )?;
        stmt.execute(params![
            index_status.file_path,
            index_status.layer.to_string(),
            index_status.file_hash,
            index_status.mtime,
            index_status.ctime,
            index_status.size,
            index_status.indexed_at
        ])?;
        Ok(())
    }

    pub fn delete_index_status_by_layer(&self, layer: IndexType) -> Result<()> {
        let conn = self.open()?;
        let mut stmt = conn.prepare("DELETE FROM index_status WHERE layer = ?1")?;
        stmt.execute(params![layer.to_string()])?;
        Ok(())
    }

    pub fn delete_index_status_by_path(&self, file_path: &str, layer: IndexType) -> Result<()> {
        let conn = self.open()?;
        let mut stmt =
            conn.prepare("DELETE FROM index_status WHERE file_path = ?1 AND layer = ?2")?;
        stmt.execute(params![file_path, layer.to_string()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test::utils::temp_dir;

    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs()
    }

    fn setup_store() -> IndexStatusStore {
        let db_path = temp_dir().join(format!("db/test_{}.db", uuid::Uuid::new_v4()));
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).expect("create db parent dir for tests");
        }
        IndexStatusStore::new(db_path)
    }

    #[test]
    fn test_create_project() {
        let project_path1 = temp_dir().join("projects/hmosworld/commons/aspect");
        let project_path2 = temp_dir().join("projects/hmosworld/commons/audioplayer");
        let store1 = setup_store();
        let store2 = setup_store();
        let project1 = store1
            .get_or_create_project(&project_path1, "veso")
            .unwrap();
        let project2 = store2
            .get_or_create_project(&project_path2, "veso")
            .unwrap();
        assert_eq!(project1.root_path, project_path1);
        assert_eq!(project2.root_path, project_path2);
        assert_eq!(project1.embedding_model, "veso");
        assert_eq!(project2.embedding_model, "veso");
    }

    #[test]
    fn test_delete_project_clears_row() {
        let store = setup_store();
        let project_path = temp_dir().join("projects/hmosworld/commons/aspect");
        store.get_or_create_project(&project_path, "veso").unwrap();
        store.delete_project(&project_path, "veso").unwrap();
        let project = store.get_project(&project_path, "veso").unwrap();
        assert!(project.is_none());
    }

    #[test]
    fn test_delete_project_by_path() {
        let store = setup_store();
        let project_path = temp_dir().join("projects/hmosworld/commons/aspect");
        store.get_or_create_project(&project_path, "veso").unwrap();
        store.delete_project(&project_path, "veso").unwrap();
        let project = store.get_project(&project_path, "veso").unwrap();
        assert!(project.is_none());
    }

    #[test]
    fn test_delete_all_projects() {
        let store = setup_store();
        let project_path1 = temp_dir().join("projects/hmosworld/commons/aspect");
        let project_path2 = temp_dir().join("projects/hmosworld/commons/audioplayer");
        store.get_or_create_project(&project_path1, "veso").unwrap();
        store.delete_all_projects().unwrap();
        let project1 = store.get_project(&project_path1, "veso").unwrap();
        assert!(project1.is_none());
        store.get_or_create_project(&project_path2, "veso").unwrap();
        store.delete_all_projects().unwrap();
        let project2 = store.get_project(&project_path2, "veso").unwrap();
        assert!(project2.is_none());
    }

    #[test]
    fn test_insert_and_delete_project_index_status() {
        let store = setup_store();
        let project_path = temp_dir().join("projects/hmosworld/commons/aspect");
        store.get_or_create_project(&project_path, "veso").unwrap();
        let index_status = IndexStatus {
            file_path: "test.txt".to_string(),
            layer: IndexType::File,
            file_hash: "test".to_string(),
            mtime: now(),
            ctime: now(),
            size: 100,
            indexed_at: now(),
        };
        store.upsert_index_status(&index_status).unwrap();
        let index_status = store.get_index_status_by_layer(IndexType::File).unwrap();
        assert_eq!(index_status.len(), 1);
        assert_eq!(index_status[0].file_path, "test.txt");
        assert_eq!(index_status[0].layer, IndexType::File);
        assert_eq!(index_status[0].file_hash, "test");

        store.delete_project(&project_path, "veso").unwrap();
        let index_status = store.get_index_status_by_layer(IndexType::File).unwrap();
        assert!(index_status.is_empty());
    }

    #[test]
    fn test_update_project() {
        let store = setup_store();
        let project_path = temp_dir().join("projects/hmosworld/commons/aspect");
        let project = store.get_or_create_project(&project_path, "veso").unwrap();
        assert_eq!(project.index_finished_time, None);
        store.update_project_index_finished_time(64).unwrap();
        let project = store.get_project(&project_path, "veso").unwrap();
        assert!(project.is_some());
        assert_eq!(project.unwrap().index_finished_time, Some(64));
    }

    #[test]
    fn test_delete_file() {
        let store = setup_store();
        let project_path = temp_dir().join("projects/hmosworld/commons/aspect");
        let project = store.get_or_create_project(&project_path, "veso").unwrap();
        let index_status = IndexStatus {
            file_path: "test.txt".to_string(),
            layer: IndexType::File,
            file_hash: "test".to_string(),
            mtime: now(),
            ctime: now(),
            size: 100,
            indexed_at: now(),
        };
        store.upsert_index_status(&index_status).unwrap();
        store
            .delete_index_status_by_path(&index_status.file_path, IndexType::File)
            .unwrap();
        let index_status = store
            .get_index_status_by_path(&index_status.file_path, IndexType::File)
            .unwrap();
        assert!(index_status.is_none());
        let _ = project;
    }

    #[test]
    fn test_upsert_update() {
        let store = setup_store();
        let project_path = temp_dir().join("projects/hmosworld/commons/aspect");
        store.get_or_create_project(&project_path, "veso").unwrap();
        let index_status = IndexStatus {
            file_path: "test.txt".to_string(),
            layer: IndexType::File,
            file_hash: "test".to_string(),
            mtime: now(),
            ctime: now(),
            size: 100,
            indexed_at: now(),
        };
        store.upsert_index_status(&index_status).unwrap();
        let index_status = store
            .get_index_status_by_path(&index_status.file_path, IndexType::File)
            .unwrap();
        assert!(index_status.is_some());
        assert_eq!(index_status.unwrap().file_hash, "test");
        let index_status = IndexStatus {
            file_path: "test.txt".to_string(),
            layer: IndexType::File,
            file_hash: "test2".to_string(),
            mtime: now(),
            ctime: now(),
            size: 100,
            indexed_at: now(),
        };
        store.upsert_index_status(&index_status).unwrap();
        let index_status = store
            .get_index_status_by_path(&index_status.file_path, IndexType::File)
            .unwrap();
        assert!(index_status.is_some());
        assert_eq!(index_status.unwrap().file_hash, "test2");
    }
}
