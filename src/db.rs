use rusqlite::{Connection, Result, params};

#[derive(Debug)]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub body: Option<String>,
}

/// Open (or create) the database file and make sure the schema exists.
pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS notes (
            id    INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            body  TEXT
        )",
        (), // no parameters
    )?;
    Ok(conn)
}

/// Insert a note and return its new row id.
pub fn add_note(conn: &Connection, title: &str, body: Option<&str>) -> Result<i64> {
    // `?1`, `?2` are bound parameters: never format values into the SQL string.
    conn.execute(
        "INSERT INTO notes (title, body) VALUES (?1, ?2)",
        params![title, body],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fetch all notes, mapping each row to a `Note`.
pub fn list_notes(conn: &Connection) -> Result<Vec<Note>> {
    let mut stmt = conn.prepare("SELECT id, title, body FROM notes ORDER BY id")?;
    let rows = stmt.query_map((), |row| {
        Ok(Note {
            id: row.get(0)?,
            title: row.get(1)?,
            body: row.get(2)?, // NULL becomes None
        })
    })?;
    rows.collect()
}

/// Delete a note; returns how many rows were removed (0 or 1).
pub fn delete_note(conn: &Connection, id: i64) -> Result<usize> {
    conn.execute("DELETE FROM notes WHERE id = ?1", params![id])
}
