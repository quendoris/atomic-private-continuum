#![forbid(unsafe_code)]

use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::Duration;

use apc_core::{commit_durable, DurabilityBackend};
use apc_storage_fs::UnixFsDurabilityBackend;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args();
    let _program = args.next();
    let store = args.next().ok_or("missing store path")?;
    let marker = args.next().ok_or("missing marker path")?;
    let stage = args.next().ok_or("missing crash stage")?;
    let payload = args.next().ok_or("missing payload")?.into_bytes();
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }

    let mut backend = UnixFsDurabilityBackend::open(&store)?;

    if stage == "after-ack" {
        commit_durable(&mut backend, &payload)?;
        signal_and_wait(Path::new(&marker));
    }

    let candidate = backend.write_candidate(&payload)?;
    if stage == "after-write" {
        signal_and_wait(Path::new(&marker));
    }

    backend.sync_candidate(&candidate)?;
    if stage == "after-candidate-sync" {
        signal_and_wait(Path::new(&marker));
    }

    backend.publish_candidate(&candidate)?;
    if stage == "after-publish" {
        signal_and_wait(Path::new(&marker));
    }

    backend.sync_committed_root()?;
    if stage == "after-root-sync" {
        signal_and_wait(Path::new(&marker));
    }

    Err(format!("unknown crash stage: {stage}").into())
}

fn signal_and_wait(marker: &Path) -> ! {
    let mut file = File::create(marker).expect("create crash-stage marker");
    file.write_all(b"ready\n")
        .expect("write crash-stage marker");
    file.sync_all().expect("sync crash-stage marker");
    drop(file);

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
