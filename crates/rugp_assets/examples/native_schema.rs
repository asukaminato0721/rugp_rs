fn main() -> anyhow::Result<()> {
    let root = std::env::args_os().nth(1).expect("game directory");
    let schema =
        rugp_assets::native_schema::NativeSchema::from_directory(std::path::Path::new(&root))?;
    println!("{} messages", schema.messages.len());
    for (name, error) in &schema.unresolved {
        eprintln!("Unresolved {name}: {error}");
    }
    for (name, fields) in schema.messages {
        if std::env::args()
            .nth(2)
            .is_none_or(|filter| name.contains(&filter))
        {
            println!("{name}: {fields:?}");
        }
    }
    Ok(())
}
