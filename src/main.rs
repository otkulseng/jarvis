mod db;

fn main() -> rusqlite::Result<()> {
    // Use ":memory:" instead of a path for a throwaway in-memory database.
    let conn = db::open("jarvis.db")?;

    let id = db::add_note(&conn, "Groceries", Some("milk, eggs"))?;
    db::add_note(&conn, "Empty note", None)?;

    for note in db::list_notes(&conn)? {
        let body = note.body.as_deref().unwrap_or("(empty)");
        println!("#{} {}: {}", note.id, note.title, body);
    }

    let removed = db::delete_note(&conn, id)?;
    println!("deleted {removed} row(s)");

    Ok(())
}
