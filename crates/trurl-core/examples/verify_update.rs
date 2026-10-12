//! Check a release file against its updater signature, exactly as installed
//! copies of Trurl will. The release workflow runs this on every file before
//! publishing, so a wrong key or a mismatched version can never reach learners.
//!
//!   cargo run -p trurl-core --example verify_update -- FILE FILE.sig VERSION [PUBKEY]
//!
//! PUBKEY defaults to the key built into Trurl (crates/trurl-core/src/update.rs).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: verify_update FILE FILE.sig VERSION [PUBKEY]");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1]).expect("read file");
    let sig = std::fs::read_to_string(&args[2]).expect("read signature");
    let pubkey = args
        .get(4)
        .map(String::as_str)
        .unwrap_or(trurl_core::update::PUBKEY);
    match trurl_core::update::verify(&bytes, &sig, pubkey, Some(&args[3])) {
        Ok(()) => println!("ok   {} is signed for version {}", args[1], args[3]),
        Err(e) => {
            eprintln!("FAIL {}: {e}", args[1]);
            std::process::exit(1);
        }
    }
}
