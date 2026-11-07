//! Probe: is the linked SQLite actually SQLCipher with working codec?
//! Run: cargo run -p goon-vault --example sqlcipher_probe

use rusqlite::Connection;

fn main() {
    let dir = std::env::temp_dir().join(format!("scq-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("t.db");

    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "PRAGMA key = \"x'0000000000000000000000000000000000000000000000000000000000000001'\";",
    )
    .unwrap();

    let ver: String = conn
        .query_row("PRAGMA cipher_version", [], |r| r.get(0))
        .unwrap_or_else(|e| format!("ERR {e}"));
    println!("cipher_version = {ver}");

    conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (42);")
        .unwrap();
    drop(conn);

    let bytes = std::fs::read(&db).unwrap();
    let header: String = bytes[..16].iter().map(|b| *b as char).collect();
    println!("header = {header:?}");
    println!(
        "plaintext magic present = {}",
        header == "SQLite format 3\0"
    );

    // Reopen with the WRONG key — must fail on the first real query.
    let conn2 = Connection::open(&db).unwrap();
    conn2
        .execute_batch(
            "PRAGMA key = \"x'0000000000000000000000000000000000000000000000000000000000000002'\";",
        )
        .unwrap();
    match conn2.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
        Ok(n) => println!("WRONG KEY OPENED DB! rows={n}"),
        Err(e) => println!("wrong key rejected: {e}"),
    }
    match conn2.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0)) {
        Ok(v) => println!("WRONG KEY READ DATA! x={v}"),
        Err(e) => println!("data unreadable with wrong key: {e}"),
    }

    std::fs::remove_dir_all(&dir).ok();
}
