use super::*;
use std::path::{Path, PathBuf};

struct ProcFixture(PathBuf);

impl ProcFixture {
    fn new(pids: &[i32]) -> Self {
        let root = std::env::temp_dir().join(format!("cma-proc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for pid in pids {
            std::fs::create_dir(root.join(pid.to_string())).unwrap();
        }
        Self(root)
    }
}

impl Drop for ProcFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pid(path: &Path) -> i32 {
    assert_eq!(path.file_name().unwrap(), "stat");
    path.parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn vanished_processes_do_not_hide_remaining_writers() {
    let fixture = ProcFixture::new(&[41, 42, 43, 44]);
    let table = process_table_from(&fixture.0, |path| match pid(path) {
        41 => Err(std::io::Error::from_raw_os_error(libc::ESRCH)),
        42 => Err(std::io::Error::from_raw_os_error(libc::ENOENT)),
        43 => Ok("43 (writer (with spaces)) S 1 43 43".into()),
        44 => Ok("44 (child) R 43 44 44".into()),
        _ => unreachable!(),
    })
    .expect("a process can exit between /proc enumeration and reading its stat");
    assert_eq!(table.len(), 2);
    let writers = writer_states(&table, &BTreeSet::from([43]), None);
    assert_eq!(writers, BTreeMap::from([(43, 'S'), (44, 'R')]));
}

#[test]
fn inspection_failures_still_prevent_a_writer_fence() {
    let fixture = ProcFixture::new(&[43]);
    for code in [libc::EACCES, libc::EPERM, libc::EIO] {
        let error =
            process_table_from(&fixture.0, |_| Err(std::io::Error::from_raw_os_error(code)))
                .unwrap_err();
        assert_eq!(
            error,
            format!(
                "inspect process 43: {}",
                std::io::Error::from_raw_os_error(code)
            )
        );
    }
    let error = process_table_from(&fixture.0, |_| Ok("malformed stat".into())).unwrap_err();
    assert_eq!(error, "process 43 has malformed stat");
}
