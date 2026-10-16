fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    match cmd.as_str() {
        "doctor" => doctor(),
        "backup" => backup(),
        "serve" | _ => {
            if let Err(err) = omabring_web::serve() {
                eprintln!("OmaBring failed: {err}");
                std::process::exit(1);
            }
        }
    }
}

fn doctor() {
    let dir = omabring_core::db::default_data_dir();
    match omabring_core::Db::open(&dir) {
        Ok(db) => {
            println!("OmaBring");
            println!("data {}", dir.display());
            println!("database {}", db.path().display());
            println!("ok");
        }
        Err(err) => {
            eprintln!("OmaBring doctor: {err}");
            std::process::exit(1);
        }
    }
}

fn backup() {
    let dir = omabring_core::db::default_data_dir();
    let db = match omabring_core::Db::open(&dir) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    match db.backup_configured() {
        Ok(path) => println!("{}", path.display()),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
