pub mod bin;
pub mod paths;
pub const TESTED_OMP_VERSION: &str = "18.14.0";
pub mod bucket;
pub mod continuation;
pub mod handle;
pub mod items;
pub mod lease;
pub mod listing;
pub mod session;

#[cfg(test)]
pub(crate) mod fixtures {
    pub fn bytes(key: char) -> &'static [u8] {
        match key {
            'A' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl"
            ),
            'B' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-alpha/2026-10-02T14-30-00-000Z_01a0fd05-a240-7c41-9d02-7d2e8f13a5b6.jsonl"
            ),
            'C' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-alpha/2026-10-02T15-10-00-000Z_01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81.jsonl"
            ),
            'G' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-alpha/2026-10-05T16-20-00-000Z_01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07.jsonl"
            ),
            'F' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-beta/2026-09-15T07-20-00-000Z_01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de.jsonl"
            ),
            'D' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-beta/2026-10-03T08-00-00-000Z_01a100c6-f000-7b18-8e6a-5e8b1c4f9a72.jsonl"
            ),
            'E' => include_bytes!(
                "../../tests/fixtures/omp/agent/sessions/-work-beta/2026-10-04T11-45-00-000Z_01a106bb-4a60-7f92-b0c3-b2d6e0a3f417.jsonl"
            ),
            _ => panic!("unknown fixture"),
        }
    }
    pub fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        copy(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/omp/agent/sessions"),
            t.path(),
        );
        t
    }
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                std::fs::create_dir(&p).unwrap();
                copy(&e.path(), &p)
            } else {
                std::fs::copy(e.path(), p).unwrap();
            }
        }
    }
    pub fn path(t: &tempfile::TempDir, key: char) -> std::path::PathBuf {
        let s =
            super::session::Session::parse(std::path::Path::new("fixture"), bytes(key)).unwrap();
        super::listing::discover(t.path())
            .unwrap()
            .into_iter()
            .find(|f| super::listing::scan_head(f.clone()).unwrap().id == s.header.id)
            .unwrap()
            .path
    }
}
