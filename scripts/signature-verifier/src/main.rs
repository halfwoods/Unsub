use std::{env, error::Error, fs::File, io::Read};

use minisign_verify::{PublicKey, Signature};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: unsub-signature-verifier PAYLOAD PUBLIC_KEY SIGNATURE".into());
    }
    let key = PublicKey::from_file(&args[1])?;
    let signature = Signature::from_file(&args[2])?;
    let mut verifier = key.verify_stream(&signature)?;
    let mut file = File::open(&args[0])?;
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        verifier.update(&buffer[..count]);
    }
    verifier.finalize()?;
    println!("Verified payload signature and trusted comment");
    Ok(())
}
