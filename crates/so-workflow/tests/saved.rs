#![allow(clippy::unwrap_used)]

use std::fs;

use so_workflow::{SavedSource, SavedWorkflows, WorkflowError};

#[test]
fn discovery_precedence_and_load() {
    let cfg = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let proj = cwd.path().join(".superorchestrator/workflows");
    fs::create_dir_all(&proj).unwrap();
    fs::write(cfg.path().join("audit.js"), "return 'config';").unwrap();
    fs::write(cfg.path().join("only-config.js"), "return 1;").unwrap();
    fs::write(cfg.path().join("notes.txt"), "ignored").unwrap();
    fs::write(proj.join("audit.js"), "return 'project';").unwrap();
    fs::write(proj.join("only-project.js"), "return 2;").unwrap();

    let saved = SavedWorkflows::new(cfg.path(), cwd.path());
    let list = saved.list();
    let names: Vec<_> = list.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["audit", "only-config", "only-project"]);
    assert_eq!(list[0].source, SavedSource::Project);
    assert_eq!(list[1].source, SavedSource::Config);

    let (w, src) = saved.load("audit").unwrap();
    assert_eq!(w.source, SavedSource::Project);
    assert_eq!(src, "return 'project';");
    assert_eq!(saved.load("only-config").unwrap().1, "return 1;");
    assert!(matches!(
        saved.load("nope"),
        Err(WorkflowError::NotFound(_))
    ));
    assert!(matches!(
        saved.load("../x"),
        Err(WorkflowError::NotFound(_))
    ));
}

#[test]
fn missing_dirs_are_empty() {
    let saved = SavedWorkflows::new("/nonexistent/a", "/nonexistent/b");
    assert!(saved.list().is_empty());
}

#[test]
fn discovery_skips_symlinks_in_both_directories() {
    let cfg = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("external.js");
    fs::write(&target, "return 'external';").unwrap();
    let proj = cwd.path().join(".superorchestrator/workflows");
    fs::create_dir_all(&proj).unwrap();
    for dir in [cfg.path(), proj.as_path()] {
        std::os::unix::fs::symlink(&target, dir.join("linked.js")).unwrap();
        std::os::unix::fs::symlink(external.path().join("missing"), dir.join("broken.js")).unwrap();
        fs::write(dir.join("regular.js"), "return 1;").unwrap();
    }
    // A project link must not hide a regular config workflow.
    fs::write(cfg.path().join("fallback.js"), "return 'config';").unwrap();
    std::os::unix::fs::symlink(&target, proj.join("fallback.js")).unwrap();
    let saved = SavedWorkflows::new(cfg.path(), cwd.path());
    let names: Vec<_> = saved.list().into_iter().map(|w| w.name).collect();
    assert_eq!(names, ["fallback", "regular"]);
    assert!(matches!(
        saved.load("linked"),
        Err(WorkflowError::NotFound(_))
    ));
    assert_eq!(saved.load("fallback").unwrap().1, "return 'config';");
}
