use super::*;
use std::fs;
use uuid::Uuid;

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> PathBuf {
    let root = std::env::temp_dir().join(format!("vibra-git-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Vibra Test"]);
    git(&root, &["config", "user.email", "vibra@example.invalid"]);
    fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
    git(&root, &["add", "tracked.txt"]);
    git(&root, &["commit", "-qm", "initial"]);
    root
}

fn capture_index_path(port: &GitCliPort, root: &Path) -> PathBuf {
    let slot = Arc::clone(
        &port
            .capture_indexes
            .lock()
            .unwrap()
            .get(root)
            .unwrap()
            .index,
    );
    slot.lock().unwrap().as_ref().unwrap().temporary.0.clone()
}

#[test]
fn numstat_parses_nul_delimited_paths_and_renames() {
    let mut stats = HashMap::new();
    apply_numstat(
        b"2\t1\todd\tname\n.rs\0\
          3\t4\t\0old.txt\0new\tname.txt\0",
        &mut stats,
    );
    assert_eq!(stats.get("odd\tname\n.rs"), Some(&(2, 1)));
    assert_eq!(stats.get("new\tname.txt"), Some(&(3, 4)));
}

#[test]
fn git_error_reader_drains_input_without_retaining_it_all() {
    let mut input = std::io::Cursor::new(vec![b'x'; MAX_GIT_ERROR_BYTES + 1024]);
    let error = read_git_error(&mut input).unwrap();
    assert_eq!(error.len(), MAX_GIT_ERROR_BYTES);
    assert_eq!(input.position(), (MAX_GIT_ERROR_BYTES + 1024) as u64);
}

#[test]
fn branch_changes_preserve_paths_with_tabs_and_newlines() {
    let root = repository();
    let name = "odd\tname\n.rs";
    fs::write(root.join(name), "before\n").unwrap();
    git(&root, &["add", "--", name]);
    git(&root, &["commit", "-qm", "add odd path"]);
    fs::write(root.join(name), "after\nextra\n").unwrap();

    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == name)
        .unwrap();
    assert_eq!((change.additions, change.deletions), (Some(2), Some(1)));

    let branch = port
        .branch_changes(&root, Some("HEAD"), None)
        .unwrap()
        .unwrap();
    let change = branch
        .snapshot
        .changes
        .iter()
        .find(|change| change.path == name)
        .unwrap();
    assert_eq!((change.additions, change.deletions), (Some(2), Some(1)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_diff_uses_a_literal_pathspec() {
    let root = repository();
    let literal = "literal[1].txt";
    fs::write(root.join(literal), "before literal\n").unwrap();
    fs::write(root.join("literal1.txt"), "before other\n").unwrap();
    git(&root, &["add", "--", literal, "literal1.txt"]);
    git(&root, &["commit", "-qm", "add pathspec fixtures"]);
    fs::write(root.join(literal), "changed literal\n").unwrap();
    fs::write(root.join("literal1.txt"), "changed other\n").unwrap();

    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == literal)
        .unwrap();
    let diff = port.diff(&root, change).unwrap();
    assert!(diff.rows.iter().any(|row| row.text == "changed literal"));
    assert!(!diff.rows.iter().any(|row| row.text == "changed other"));

    git(&root, &["add", "--", literal, "literal1.txt"]);
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let staged = snapshot
        .changes
        .iter()
        .find(|change| change.path == literal)
        .unwrap();
    let diff = port.diff(&root, staged).unwrap();
    assert!(diff.rows.iter().any(|row| row.text == "changed literal"));
    assert!(!diff.rows.iter().any(|row| row.text == "changed other"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn branch_changes_preserve_renamed_paths_with_control_characters() {
    let root = repository();
    let renamed = "renamed\tfile\n.txt";
    git(&root, &["mv", "tracked.txt", renamed]);

    let branch = GitCliPort::default()
        .branch_changes(&root, Some("HEAD"), None)
        .unwrap()
        .unwrap();
    let change = branch
        .snapshot
        .changes
        .iter()
        .find(|change| change.path == renamed)
        .unwrap();
    assert_eq!(change.status, GitFileStatus::Renamed);
    assert_eq!((change.additions, change.deletions), (Some(0), Some(0)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_and_diff_cover_staged_unstaged_and_untracked_files() {
    let root = repository();
    fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
    fs::write(root.join("staged.txt"), "prepared\n").unwrap();
    git(&root, &["add", "staged.txt"]);
    fs::write(root.join("new file.txt"), "new\nfile\n").unwrap();
    let port = GitCliPort::default();

    let snapshot = port.snapshot(&root).unwrap().unwrap();

    assert_eq!(snapshot.changes.len(), 3);
    assert!(snapshot.changes.iter().any(|change| {
        change.path == "tracked.txt" && change.unstaged && change.deletions == Some(1)
    }));
    assert!(snapshot.changes.iter().any(|change| {
        change.path == "staged.txt" && change.staged && change.status == GitFileStatus::Added
    }));
    let untracked = snapshot
        .changes
        .iter()
        .find(|change| change.path == "new file.txt")
        .unwrap();
    assert_eq!(untracked.additions, Some(2));
    assert_eq!(untracked.deletions, Some(0));
    let diff = port.diff(&root, untracked).unwrap();
    assert_eq!(diff.additions, 2);
    assert!(diff.rows.iter().any(|row| row.text == "new"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn staged_deletion_and_untracked_recreation_share_one_reviewable_path() {
    let root = repository();
    fs::remove_file(root.join("tracked.txt")).unwrap();
    git(&root, &["add", "-u"]);
    fs::write(root.join("tracked.txt"), "replacement\nnew\n").unwrap();

    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    assert_eq!(snapshot.changes.len(), 1);
    let change = &snapshot.changes[0];
    assert_eq!(change.path, "tracked.txt");
    assert!(change.staged && change.untracked);
    assert_eq!((change.additions, change.deletions), (Some(2), Some(2)));
    let diff = port.diff(&root, change).unwrap();
    assert!(diff.rows.iter().any(|row| row.text == "STAGED CHANGES"));
    assert!(diff.rows.iter().any(|row| row.text == "UNTRACKED"));
    assert!(diff.rows.iter().any(|row| row.text == "replacement"));
    assert!(diff.rows.iter().any(|row| row.text == "one"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn untracked_line_counts_show_before_the_diff_is_opened() {
    let root = repository();
    fs::write(root.join("plain.txt"), "one\ntwo\nthree").unwrap();
    fs::write(root.join("empty.txt"), "").unwrap();
    fs::write(root.join("binary.bin"), b"hi\0there\n").unwrap();
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    let port = GitCliPort::default();

    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let additions = |path: &str| {
        snapshot
            .changes
            .iter()
            .find(|change| change.path == path)
            .unwrap()
            .additions
    };
    assert_eq!(additions("plain.txt"), Some(3));
    assert_eq!(additions("empty.txt"), Some(0));
    assert_eq!(additions("binary.bin"), None);
    assert_eq!(additions("nested/lib.rs"), Some(2));
    assert_eq!(snapshot.additions, 5);

    fs::write(root.join("plain.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let refreshed = port.snapshot(&root).unwrap().unwrap();
    assert_eq!(
        refreshed
            .changes
            .iter()
            .find(|change| change.path == "plain.txt")
            .unwrap()
            .additions,
        Some(4)
    );

    let changes = port.branch_changes(&root, None, None).unwrap().unwrap();
    assert_eq!(
        changes
            .snapshot
            .changes
            .iter()
            .find(|change| change.path == "nested/lib.rs")
            .unwrap()
            .additions,
        Some(2)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn untracked_line_count_refreshes_after_same_size_and_mtime_edit() {
    let root = std::env::temp_dir().join(format!("vibra-stat-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("new.txt");
    fs::write(&path, "abc").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let mut cache = HashMap::new();
    assert_eq!(cached_untracked_additions(&path, &mut cache), Some(1));

    std::thread::sleep(Duration::from_millis(20));
    fs::write(&path, "a\nb").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(after.modified().unwrap(), modified);
    assert_eq!(cached_untracked_additions(&path, &mut cache), Some(2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn text_additions_match_a_new_file_diff() {
    assert_eq!(text_additions(b""), Some(0));
    assert_eq!(text_additions(b"one\n"), Some(1));
    assert_eq!(text_additions(b"one\ntwo"), Some(2));
    assert_eq!(text_additions(b"one\ntwo\n"), Some(2));
    assert_eq!(text_additions(b"hi\0there\n"), None);
    let mut late_nul = vec![b'a'; 8_001];
    late_nul[8_000] = 0;
    assert_eq!(text_additions(&late_nul), Some(1));
}

#[test]
fn untracked_files_inside_new_directories_are_listed() {
    let root = repository();
    fs::create_dir_all(root.join("new_module/src")).unwrap();
    fs::write(root.join("new_module/src/lib.rs"), "pub fn n() {}\n").unwrap();
    fs::write(root.join("new_module/README.md"), "new\n").unwrap();
    let port = GitCliPort::default();

    let snapshot = port.snapshot(&root).unwrap().unwrap();
    assert!(
        snapshot
            .changes
            .iter()
            .any(|change| { change.path == "new_module/src/lib.rs" && change.untracked }),
        "worktree snapshot should list files inside untracked directories, got {:?}",
        snapshot
            .changes
            .iter()
            .map(|change| change.path.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        snapshot
            .changes
            .iter()
            .any(|change| change.path == "new_module/README.md" && change.untracked)
    );
    assert!(
        !snapshot
            .changes
            .iter()
            .any(|change| change.path == "new_module" || change.path == "new_module/")
    );

    let lib = snapshot
        .changes
        .iter()
        .find(|change| change.path == "new_module/src/lib.rs")
        .unwrap();
    let diff = port.diff(&root, lib).unwrap();
    assert!(diff.rows.iter().any(|row| row.text.contains("pub fn n()")));

    let changes = port.branch_changes(&root, None, None).unwrap().unwrap();
    assert!(
        changes
            .snapshot
            .changes
            .iter()
            .any(|change| { change.path == "new_module/src/lib.rs" && change.untracked }),
        "branch changes should list untracked files inside new directories, got {:?}",
        changes
            .snapshot
            .changes
            .iter()
            .map(|change| change.path.as_str())
            .collect::<Vec<_>>()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn untracked_nested_repository_diff_does_not_fail() {
    let root = repository();
    let nested = root.join("vendor/other");
    fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    git(&nested, &["config", "user.name", "Vibra Test"]);
    git(&nested, &["config", "user.email", "vibra@example.invalid"]);
    fs::write(nested.join("x.txt"), "x\n").unwrap();
    git(&nested, &["add", "x.txt"]);
    git(&nested, &["commit", "-qm", "nested"]);
    let port = GitCliPort::default();

    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let nested_change = snapshot
        .changes
        .iter()
        .find(|change| change.path == "vendor/other" || change.path.starts_with("vendor/other/"))
        .unwrap_or_else(|| {
            panic!(
                "expected nested untracked repo, got {:?}",
                snapshot
                    .changes
                    .iter()
                    .map(|change| change.path.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert!(nested_change.untracked);
    let diff = port.diff(&root, nested_change).unwrap();
    assert!(
        diff.rows
            .iter()
            .any(|row| row.kind == GitDiffRowKind::Notice)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn git_program_can_run_version() {
    assert!(
        git_is_usable(git_program()),
        "resolved Git should be executable: {}",
        git_program().display()
    );
}

#[test]
fn discover_skips_unusable_binaries_and_finds_a_working_git() {
    let found = discover_git_with(
        Some(PathBuf::from("/definitely/missing/vibra-git")),
        Some("/usr/bin:/bin:/usr/sbin:/sbin".into()),
        extra_git_candidates(),
    );
    assert!(
        git_is_usable(&found),
        "should resolve Homebrew or PATH Git, got {}",
        found.display()
    );
    if !git_is_usable(Path::new("/usr/bin/git")) {
        assert_ne!(
            found,
            PathBuf::from("/usr/bin/git"),
            "must not use the broken Xcode git stub"
        );
    }
}

#[test]
fn missing_repository_is_none_not_an_error() {
    let root = std::env::temp_dir().join(format!("vibra-not-git-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let snapshot = GitCliPort::default().snapshot(&root).unwrap();
    assert!(snapshot.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn not_a_repository_message_is_detected() {
    assert!(is_not_a_repository(
        b"fatal: not a git repository (or any of the parent directories): .git\n"
    ));
    assert!(!is_not_a_repository(
        b"You have not agreed to the Xcode license agreements."
    ));
}

#[test]
fn snapshot_resolves_the_repository_from_a_nested_working_directory() {
    let root = repository();
    let nested = root.join("src/deep");
    fs::create_dir_all(&nested).unwrap();

    let snapshot = GitCliPort::default().snapshot(&nested).unwrap().unwrap();

    assert_eq!(snapshot.root, root.canonicalize().unwrap());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn branch_summary_reports_dirty_and_tracking_without_full_snapshot() {
    let root = repository();
    let clean = GitCliPort::default()
        .branch_summary(&root)
        .unwrap()
        .unwrap();
    assert!(!clean.dirty);
    assert!(clean.branch == "main" || clean.branch == "master");

    fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
    let dirty = GitCliPort::default()
        .branch_summary(&root)
        .unwrap()
        .unwrap();
    assert!(dirty.dirty);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_seeds_the_branch_summary_cache() {
    let root = repository();
    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    assert!(snapshot.changes.is_empty());

    fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
    let cached = port.branch_summary(&root).unwrap().unwrap();
    assert_eq!(cached.branch, snapshot.branch);
    assert!(
        !cached.dirty,
        "branch_summary should reuse the snapshot cache within the TTL"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn hunk_parser_tracks_old_and_new_line_numbers() {
    let mut rows = Vec::new();
    let mut additions = 0;
    let mut deletions = 0;
    let mut binary = false;
    let mut truncated = false;
    append_patch(
        b"@@ -4,2 +4,2 @@\n-old\n+new\n context\n",
        None,
        &mut rows,
        &mut additions,
        &mut deletions,
        &mut binary,
        &mut truncated,
    );

    assert_eq!(rows[1].old_line, Some(4));
    assert_eq!(rows[2].new_line, Some(4));
    assert_eq!((additions, deletions), (1, 1));
}

#[test]
fn history_lists_commits_newest_first_with_parents() {
    let root = repository();
    fs::write(root.join("tracked.txt"), "one\ntwo\nthree\n").unwrap();
    git(&root, &["add", "tracked.txt"]);
    git(&root, &["commit", "-qm", "second"]);
    let history = GitCliPort::default().history(&root, 20).unwrap().unwrap();

    assert_eq!(history.total, 2);
    assert_eq!(history.commits.len(), 2);
    assert_eq!(history.commits[0].subject, "second");
    assert_eq!(history.commits[1].subject, "initial");
    assert_eq!(
        history.commits[0].parents,
        vec![history.commits[1].sha.clone()]
    );
    assert!(history.commits[1].parents.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn commit_changes_pin_the_selected_commit_and_leave_local_changes_alone() {
    let root = repository();
    let parent = rev_parse(&root, "HEAD").unwrap().unwrap();
    fs::write(root.join("tracked.txt"), "one\ncommitted\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "selected"]);
    let selected = rev_parse(&root, "HEAD").unwrap().unwrap();
    fs::write(root.join("tracked.txt"), "later commit\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "later"]);
    fs::write(root.join("tracked.txt"), "staged\n").unwrap();
    git(&root, &["add", "."]);
    fs::write(root.join("tracked.txt"), "unstaged\n").unwrap();
    fs::write(root.join("untracked.txt"), "untracked\n").unwrap();
    let port = GitCliPort::default();
    let before = port.snapshot(&root).unwrap();
    let head_before = rev_parse(&root, "HEAD").unwrap();

    let changes = port.commit_changes(&root, &selected).unwrap();
    assert_eq!(changes.base_revision, parent);
    assert_eq!(changes.revision, selected);
    assert_eq!(changes.snapshot.changes.len(), 1);
    assert_eq!(
        (changes.snapshot.additions, changes.snapshot.deletions),
        (1, 1)
    );
    let file = &changes.snapshot.changes[0];
    assert_eq!(file.path, "tracked.txt");
    let diff = port
        .diff_against(&root, &changes.base_revision, Some(&changes.revision), file)
        .unwrap();
    assert!(
        diff.rows
            .iter()
            .any(|row| row.kind == GitDiffRowKind::Addition && row.text == "committed")
    );
    assert!(
        diff.rows
            .iter()
            .any(|row| row.kind == GitDiffRowKind::Deletion && row.text == "two")
    );
    assert_eq!((diff.additions, diff.deletions), (1, 1));
    assert_eq!(port.snapshot(&root).unwrap(), before);
    assert_eq!(rev_parse(&root, "HEAD").unwrap(), head_before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn commit_changes_support_the_initial_and_empty_commits() {
    let root = repository();
    let port = GitCliPort::default();
    let changes = port.commit_changes(&root, "HEAD").unwrap();
    assert_eq!(changes.snapshot.changes.len(), 1);
    let file = &changes.snapshot.changes[0];
    assert_eq!(file.status, GitFileStatus::Added);
    let diff = port
        .diff_against(&root, &changes.base_revision, Some(&changes.revision), file)
        .unwrap();
    assert_eq!((diff.additions, diff.deletions), (2, 0));
    assert!(
        diff.rows
            .iter()
            .any(|row| row.kind == GitDiffRowKind::Addition && row.text == "one")
    );

    git(&root, &["commit", "--allow-empty", "-qm", "empty"]);
    let empty = port.commit_changes(&root, "HEAD").unwrap();
    assert_eq!(empty.base_revision, changes.revision);
    assert!(empty.snapshot.changes.is_empty());
    assert_eq!((empty.snapshot.additions, empty.snapshot.deletions), (0, 0));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn commit_changes_compare_merges_with_the_first_parent() {
    let root = repository();
    git(&root, &["branch", "-M", "main"]);
    git(&root, &["checkout", "-qb", "feature"]);
    fs::write(root.join("feature.txt"), "merged change\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "feature"]);
    git(&root, &["checkout", "-q", "main"]);
    fs::write(root.join("main.txt"), "already on main\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "main change"]);
    let first_parent = rev_parse(&root, "HEAD").unwrap().unwrap();
    git(
        &root,
        &["merge", "--no-ff", "-qm", "merge feature", "feature"],
    );

    let port = GitCliPort::default();
    let changes = port.commit_changes(&root, "HEAD").unwrap();
    assert_eq!(changes.base_revision, first_parent);
    assert_eq!(changes.snapshot.changes.len(), 1);
    assert_eq!(changes.snapshot.changes[0].path, "feature.txt");
    let diff = port
        .diff_against(
            &root,
            &changes.base_revision,
            Some(&changes.revision),
            &changes.snapshot.changes[0],
        )
        .unwrap();
    assert_eq!((diff.additions, diff.deletions), (1, 0));
    assert!(diff.rows.iter().any(|row| row.text == "merged change"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn commit_changes_cover_renames_binary_and_literal_paths() {
    let root = repository();
    // A rename keeps both paths reviewable; null-delimited metadata preserves
    // whitespace, Unicode and pathspec characters in committed filenames.
    let renamed = "renamed\tfile\nñ.txt";
    let literal = "[literal]*.txt";
    git(&root, &["mv", "tracked.txt", renamed]);
    fs::write(root.join("binary.bin"), b"\0\x01\x02\x03").unwrap();
    fs::write(root.join(literal), "literal path\n").unwrap();
    fs::write(root.join("other.txt"), "another path\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "rename and add files"]);
    let port = GitCliPort::default();
    let changes = port.commit_changes(&root, "HEAD").unwrap();
    assert_eq!(changes.snapshot.changes.len(), 5);
    assert_eq!(
        (changes.snapshot.additions, changes.snapshot.deletions),
        (4, 2)
    );
    for file in &changes.snapshot.changes {
        let diff = port
            .diff_against(&root, &changes.base_revision, Some(&changes.revision), file)
            .unwrap();
        match file.path.as_str() {
            "tracked.txt" => {
                assert_eq!(file.status, GitFileStatus::Deleted);
                assert_eq!((diff.additions, diff.deletions), (0, 2));
            }
            "binary.bin" => {
                assert!(diff.binary);
                assert_eq!(file.additions, None);
            }
            path if path == renamed => {
                assert_eq!(file.status, GitFileStatus::Added);
                assert_eq!(file.additions, Some(2));
                assert_eq!((diff.additions, diff.deletions), (2, 0));
            }
            path if path == literal => {
                assert_eq!((diff.additions, diff.deletions), (1, 0));
                assert!(diff.rows.iter().any(|row| row.text == "literal path"));
                assert!(!diff.rows.iter().any(|row| row.text == "another path"));
            }
            _ => assert_eq!((diff.additions, diff.deletions), (1, 0)),
        }
    }
    assert!(port.commit_changes(&root, "--output=unexpected").is_err());
    assert!(port.commit_changes(&root, "missing-commit").is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn branch_changes_include_committed_files_against_the_base() {
    let root = repository();
    git(&root, &["checkout", "-qb", "feature"]);
    fs::write(root.join("feature.txt"), "on the branch\n").unwrap();
    git(&root, &["add", "feature.txt"]);
    git(&root, &["commit", "-qm", "add feature"]);
    fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();

    let changes = GitCliPort::default()
        .branch_changes(&root, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(changes.commits_ahead, 1);
    assert!(!changes.base.is_empty());
    assert!(
        changes.snapshot.changes.iter().any(|change| {
            change.path == "feature.txt" && change.status == GitFileStatus::Added
        })
    );
    assert!(
        changes
            .snapshot
            .changes
            .iter()
            .any(|change| change.path == "tracked.txt")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn branch_diff_shows_a_staged_deletion_and_recreated_untracked_file() {
    let root = repository();
    git(&root, &["rm", "--cached", "--", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "replacement\nnew\n").unwrap();

    let port = GitCliPort::default();
    let branch = port
        .branch_changes(&root, Some("HEAD"), None)
        .unwrap()
        .unwrap();
    assert_eq!(branch.snapshot.changes.len(), 1);
    let change = &branch.snapshot.changes[0];
    assert_eq!(change.path, "tracked.txt");
    assert_eq!(change.status, GitFileStatus::Deleted);
    assert!(change.untracked);
    assert_eq!((change.additions, change.deletions), (Some(2), Some(2)));

    let diff = port
        .diff_against(&root, &branch.base_revision, None, change)
        .unwrap();
    assert_eq!((diff.additions, diff.deletions), (2, 2));
    assert!(
        diff.rows
            .iter()
            .any(|row| { row.kind == GitDiffRowKind::Section && row.text == "CHANGES FROM BASE" })
    );
    assert!(
        diff.rows
            .iter()
            .any(|row| { row.kind == GitDiffRowKind::Section && row.text == "UNTRACKED" })
    );
    assert!(
        diff.rows
            .iter()
            .any(|row| { row.kind == GitDiffRowKind::Deletion && row.text == "one" })
    );
    assert!(
        diff.rows
            .iter()
            .any(|row| { row.kind == GitDiffRowKind::Addition && row.text == "replacement" })
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_captures_compare_one_turn_without_touching_the_index() {
    let root = repository();
    fs::write(root.join("before.txt"), "untracked before the turn\n").unwrap();
    fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();
    let port = GitCliPort::default();
    let status_before = port.snapshot(&root).unwrap().unwrap();

    let base = port.capture_worktree(&root).unwrap().unwrap();
    fs::write(root.join("tracked.txt"), "one\nturn\n").unwrap();
    fs::write(root.join("created.txt"), "made by the agent\n").unwrap();
    fs::write(root.join("ignored.log"), "noise\n").unwrap();
    fs::remove_file(root.join("before.txt")).unwrap();
    let head = port.capture_worktree(&root).unwrap().unwrap();

    let changes = port.tree_changes(&root, &base.tree, &head.tree).unwrap();
    let mut paths: Vec<(&str, GitFileStatus)> = changes
        .snapshot
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.status))
        .collect();
    paths.sort_by_key(|(path, _)| *path);
    assert_eq!(
        paths,
        vec![
            ("before.txt", GitFileStatus::Deleted),
            ("created.txt", GitFileStatus::Added),
            ("tracked.txt", GitFileStatus::Modified),
        ]
    );
    let tracked = changes
        .snapshot
        .changes
        .iter()
        .find(|change| change.path == "tracked.txt")
        .unwrap();
    let diff = port
        .diff_against(&root, &base.tree, Some(&head.tree), tracked)
        .unwrap();
    assert_eq!((diff.additions, diff.deletions), (1, 1));

    // The real index and status are exactly as they were.
    let status_after = port.snapshot(&root).unwrap().unwrap();
    assert!(status_after.changes.iter().all(|change| !change.staged));
    assert_eq!(status_before.branch, status_after.branch);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_capture_reuses_index_and_invalidates_after_real_stage() {
    let root = repository();
    let port = GitCliPort::default();
    let first = port.capture_worktree(&root).unwrap().unwrap();
    let cached_path = capture_index_path(&port, &first.root);
    assert_eq!(
        port.capture_worktree(&root).unwrap().unwrap().tree,
        first.tree
    );
    assert!(cached_path.is_file());

    fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
    let changed = port.capture_worktree(&root).unwrap().unwrap();
    assert_ne!(changed.tree, first.tree);
    assert_eq!(capture_index_path(&port, &first.root), cached_path);

    // A file newly forced into the real index would be ignored by the
    // private index unless its source copy is invalidated.
    fs::write(root.join(".gitignore"), "forced.txt\n").unwrap();
    fs::write(root.join("forced.txt"), "included by a real stage\n").unwrap();
    git(&root, &["add", "-f", "forced.txt"]);
    let staged = port.capture_worktree(&root).unwrap().unwrap();
    assert_ne!(staged.tree, changed.tree);
    assert_ne!(capture_index_path(&port, &first.root), cached_path);
    assert!(!cached_path.exists());

    let path = capture_index_path(&port, &first.root);
    drop(port);
    assert!(!path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_fingerprint_detects_content_change_even_with_matching_inode_size_and_mtime() {
    let root = std::env::temp_dir().join(format!("vibra-index-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let index_path = root.join("index");
    fs::write(&index_path, b"version-a").unwrap();
    let original = fs::metadata(&index_path).unwrap();
    let original_fingerprint = index_fingerprint(&index_path).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    fs::write(&index_path, b"version-b").unwrap();
    fs::File::options()
        .write(true)
        .open(&index_path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original.modified().unwrap()))
        .unwrap();
    let changed = fs::metadata(&index_path).unwrap();
    assert_eq!(changed.ino(), original.ino());
    assert_eq!(changed.len(), original.len());
    assert_eq!(
        (changed.mtime(), changed.mtime_nsec()),
        (original.mtime(), original.mtime_nsec())
    );
    assert_ne!(
        index_fingerprint(&index_path).unwrap(),
        original_fingerprint
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_captures_in_other_repositories_do_not_wait_for_an_index() {
    let first = repository();
    let second = repository();
    let port = Arc::new(GitCliPort::default());
    let captured = port.capture_worktree(&first).unwrap().unwrap();
    let first_slot = Arc::clone(
        &port
            .capture_indexes
            .lock()
            .unwrap()
            .get(&captured.root)
            .unwrap()
            .index,
    );
    let guard = first_slot.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let other_port = Arc::clone(&port);
    let other_root = second.clone();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(other_port.capture_worktree(&other_root));
    });
    let result = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("a capture in another repository must not wait for this index");
    assert!(result.unwrap().is_some());
    drop(guard);
    worker.join().unwrap();
    drop(port);
    fs::remove_dir_all(first).unwrap();
    fs::remove_dir_all(second).unwrap();
}

#[test]
fn worktree_capture_tracks_ignored_untracked_transitions() {
    let root = repository();
    fs::write(root.join("scratch.txt"), "scratch\n").unwrap();
    let port = GitCliPort::default();
    let before = port.capture_worktree(&root).unwrap().unwrap();
    fs::write(root.join(".gitignore"), "scratch.txt\n").unwrap();
    let after = port.capture_worktree(&root).unwrap().unwrap();
    let changes = port.tree_changes(&root, &before.tree, &after.tree).unwrap();
    assert!(
        changes.snapshot.changes.iter().any(|change| {
            change.path == "scratch.txt" && change.status == GitFileStatus::Deleted
        })
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_capture_keeps_a_tracked_ignored_file_after_recreation() {
    let root = repository();
    fs::write(root.join(".gitignore"), "tracked.txt\n").unwrap();
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "-qm", "ignore the already tracked path"]);
    let port = GitCliPort::default();
    let before = port.capture_worktree(&root).unwrap().unwrap();

    fs::remove_file(root.join("tracked.txt")).unwrap();
    let missing = port.capture_worktree(&root).unwrap().unwrap();
    assert_ne!(missing.tree, before.tree);
    fs::write(root.join("tracked.txt"), "restored\n").unwrap();
    let restored = port.capture_worktree(&root).unwrap().unwrap();
    let changes = port
        .tree_changes(&root, &before.tree, &restored.tree)
        .unwrap();
    assert!(changes.snapshot.changes.iter().any(|change| {
        change.path == "tracked.txt" && change.status == GitFileStatus::Modified
    }));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_capture_detects_same_size_and_mtime_edits() {
    use std::fs::FileTimes;

    let root = repository();
    git(&root, &["config", "core.trustctime", "false"]);
    let path = root.join("tracked.txt");
    fs::write(&path, "one\nfirst\n").unwrap();
    let port = GitCliPort::default();
    let before = port.capture_worktree(&root).unwrap().unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();

    fs::write(&path, "one\nother\n").unwrap();
    fs::File::open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    let after = port.capture_worktree(&root).unwrap().unwrap();
    assert_ne!(after.tree, before.tree);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn diff_sources_follow_the_compared_sides() {
    let root = repository();
    let port = GitCliPort::default();
    fs::write(root.join("tracked.txt"), "one\nchanged\n").unwrap();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let tracked = snapshot
        .changes
        .iter()
        .find(|change| change.path == "tracked.txt")
        .unwrap();
    let sources = port.diff_sources(&root, tracked, None, None).unwrap();
    assert_eq!(sources.old.as_deref(), Some("one\ntwo\n"));
    assert_eq!(sources.new.as_deref(), Some("one\nchanged\n"));

    git(&root, &["add", "tracked.txt"]);
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let staged = snapshot.changes.first().unwrap();
    let sources = port.diff_sources(&root, staged, None, None).unwrap();
    assert_eq!(sources.old.as_deref(), Some("one\ntwo\n"));
    assert_eq!(sources.new.as_deref(), Some("one\nchanged\n"));

    fs::write(root.join("tracked.txt"), "one\nchanged\nagain\n").unwrap();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let both = snapshot.changes.first().unwrap();
    assert_eq!(
        port.diff_sources(&root, both, None, None).unwrap(),
        GitDiffSources::default()
    );

    fs::write(root.join("binary.bin"), b"a\0b").unwrap();
    let binary = GitFileChange {
        path: "binary.bin".into(),
        old_path: None,
        status: GitFileStatus::Untracked,
        staged: false,
        unstaged: false,
        untracked: true,
        additions: None,
        deletions: None,
    };
    assert_eq!(
        port.diff_sources(&root, &binary, None, None).unwrap(),
        GitDiffSources::default()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn branch_selectors_list_local_and_remote_refs_without_aliases_or_tags() {
    let root = repository();
    git(&root, &["branch", "-M", "main"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        &root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    git(&root, &["branch", "origin/main"]);
    git(&root, &["tag", "main"]);
    let branches = GitCliPort::default().branches(&root).unwrap();
    assert_eq!(branches.len(), 3);
    assert!(
        branches
            .iter()
            .any(|branch| branch.reference == "refs/heads/main" && !branch.remote)
    );
    assert!(
        branches
            .iter()
            .any(|branch| branch.reference == "refs/heads/origin/main" && !branch.remote)
    );
    assert!(
        branches
            .iter()
            .any(|branch| branch.reference == "refs/remotes/origin/main" && branch.remote)
    );
    let changes = GitCliPort::default()
        .branch_changes(
            &root,
            Some("refs/remotes/origin/main"),
            Some("refs/heads/main"),
        )
        .unwrap()
        .unwrap();
    assert!(changes.snapshot.changes.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_branches_compare_both_tips_and_pin_diffs_without_checkout() {
    let root = repository();
    git(&root, &["branch", "-M", "main"]);
    git(&root, &["checkout", "-qb", "remote-tip"]);
    fs::write(root.join("remote.txt"), "remote version\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "remote change"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&root, &["checkout", "-q", "main"]);
    fs::write(root.join("local.txt"), "saved local version\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "local change"]);
    fs::write(root.join("tracked.txt"), "staged\n").unwrap();
    git(&root, &["add", "."]);
    fs::write(root.join("local.txt"), "unsaved local version\n").unwrap();
    fs::write(root.join("untracked.txt"), "untracked\n").unwrap();
    let port = GitCliPort::default();
    let before = port.snapshot(&root).unwrap();
    let changes = port
        .branch_changes(
            &root,
            Some("refs/remotes/origin/main"),
            Some("refs/heads/main"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(changes.snapshot.changes.len(), 2);
    assert_eq!(
        (changes.snapshot.additions, changes.snapshot.deletions),
        (1, 1)
    );
    let local = changes
        .snapshot
        .changes
        .iter()
        .find(|c| c.path == "local.txt")
        .unwrap();
    let remote = changes
        .snapshot
        .changes
        .iter()
        .find(|c| c.path == "remote.txt")
        .unwrap();
    assert_eq!(remote.status, GitFileStatus::Deleted);
    let diff = port
        .diff_against(
            &root,
            &changes.base_revision,
            changes.head_revision.as_deref(),
            local,
        )
        .unwrap();
    assert!(
        diff.rows
            .iter()
            .any(|row| row.text == "saved local version")
    );
    assert!(
        !diff
            .rows
            .iter()
            .any(|row| row.text == "unsaved local version")
    );
    assert_eq!(before, port.snapshot(&root).unwrap());
    let worktree = port
        .branch_changes(&root, Some("refs/remotes/origin/main"), None)
        .unwrap()
        .unwrap();
    assert!(worktree.head_revision.is_none());
    assert!(
        worktree
            .snapshot
            .changes
            .iter()
            .any(|c| c.path == "untracked.txt")
    );
    assert!(
        worktree
            .snapshot
            .changes
            .iter()
            .any(|c| c.path == "remote.txt" && c.status == GitFileStatus::Deleted)
    );
    let reverse = port
        .branch_changes(
            &root,
            Some("refs/heads/main"),
            Some("refs/remotes/origin/main"),
        )
        .unwrap()
        .unwrap();
    assert!(
        reverse
            .snapshot
            .changes
            .iter()
            .any(|c| c.path == "remote.txt" && c.status == GitFileStatus::Added)
    );
    git(&root, &["commit", "-qam", "advance local"]);
    let pinned = port
        .diff_against(
            &root,
            &changes.base_revision,
            changes.head_revision.as_deref(),
            local,
        )
        .unwrap();
    assert_eq!(pinned, diff);
    let refreshed = port
        .branch_changes(
            &root,
            Some("refs/remotes/origin/main"),
            Some("refs/heads/main"),
        )
        .unwrap()
        .unwrap();
    assert_ne!(refreshed.head_revision, changes.head_revision);
    assert!(
        port.branch_changes(&root, Some("missing"), Some("refs/heads/main"))
            .is_err()
    );
    assert!(port.branch_changes(&root, Some("--help"), None).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parse_history_preserves_control_characters_in_subjects() {
    let commits = parse_history(
        b"aaa\0aaa\0subject\x1fwith\ncontrols\0Ada\x002023-11-14\0bbb ccc\0\
          bbb\0bbb\0root\0Ada\x002023-07-22\0\0",
    );
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].subject, "subject\x1fwith\ncontrols");
    assert_eq!(commits[0].parents, vec!["bbb", "ccc"]);
    assert!(commits[1].parents.is_empty());
}

#[test]
fn renamed_file_keeps_both_paths_in_worktree_and_branch_diffs() {
    let root = repository();
    git(&root, &["mv", "tracked.txt", "moved.txt"]);
    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == "moved.txt")
        .unwrap();
    assert_eq!(change.old_path.as_deref(), Some("tracked.txt"));
    let diff = port.diff(&root, change).unwrap();
    assert_eq!((diff.additions, diff.deletions), (0, 0));
    assert!(diff.rows.iter().any(|row| row.text == "From tracked.txt"));
    let sources = port.diff_sources(&root, change, None, None).unwrap();
    assert_eq!(sources.old, sources.new);

    let base = rev_parse(&root, "HEAD").unwrap().unwrap();
    let branch = port
        .branch_changes(&root, Some(&base), None)
        .unwrap()
        .unwrap();
    let change = branch
        .snapshot
        .changes
        .iter()
        .find(|change| change.path == "moved.txt")
        .unwrap();
    assert_eq!(change.old_path.as_deref(), Some("tracked.txt"));
    let diff = port.diff_against(&root, &base, None, change).unwrap();
    assert_eq!((diff.additions, diff.deletions), (0, 0));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicted_file_shows_the_worktree_markers() {
    let root = repository();
    let original_branch = current_branch(&root).unwrap();
    git(&root, &["checkout", "-qb", "side"]);
    fs::write(root.join("tracked.txt"), "side\n").unwrap();
    git(&root, &["commit", "-qam", "side"]);
    git(&root, &["checkout", "-q", &original_branch]);
    fs::write(root.join("tracked.txt"), "main\n").unwrap();
    git(&root, &["commit", "-qam", "main"]);
    let merge = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["merge", "side"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == "tracked.txt")
        .unwrap();
    assert_eq!(change.status, GitFileStatus::Conflicted);
    let diff = port.diff(&root, change).unwrap();
    assert!(diff.rows.iter().any(|row| row.text == "<<<<<<< HEAD"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_source_does_not_follow_an_external_symlink() {
    let root = repository();
    let outside = root.with_extension("external.txt");
    fs::write(&outside, "outside secret\n").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == "link.txt")
        .unwrap();
    assert!(
        port.diff_sources(&root, change, None, None)
            .unwrap()
            .new
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
    fs::remove_file(outside).unwrap();
}

#[test]
fn local_diff_does_not_execute_textconv() {
    use std::os::unix::fs::PermissionsExt;

    let root = repository();
    let marker = root.join("textconv-ran");
    let script = root.join("textconv.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf x >> '{}'\ncat \"$1\"\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    git(
        &root,
        &["config", "diff.audit.textconv", script.to_str().unwrap()],
    );
    fs::write(root.join(".gitattributes"), "*.txt diff=audit\n").unwrap();
    fs::write(root.join("tracked.txt"), "changed\n").unwrap();
    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == "tracked.txt")
        .unwrap();
    let _ = port.diff(&root, change).unwrap();
    assert!(!marker.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_root_preserves_a_trailing_space() {
    let root = std::env::temp_dir().join(format!("vibra-git-space-{} ", Uuid::new_v4().simple()));
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    assert_eq!(
        repository_root(&root).unwrap(),
        Some(root.canonicalize().unwrap())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_diffs_are_truncated_while_reading_git_output() {
    let root = repository();
    let path = root.join("large.txt");
    fs::write(&path, vec![b'x'; MAX_DIFF_BYTES + 1024]).unwrap();
    let port = GitCliPort::default();
    let snapshot = port.snapshot(&root).unwrap().unwrap();
    let change = snapshot
        .changes
        .iter()
        .find(|change| change.path == "large.txt")
        .unwrap();

    let diff = port.diff(&root, change).unwrap();

    assert!(diff.truncated);
    assert!(diff.rows.iter().any(|row| {
        row.kind == GitDiffRowKind::Notice && row.text.contains("truncated to 4 MiB")
    }));
    fs::remove_dir_all(root).unwrap();
}
