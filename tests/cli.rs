use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "human-cli-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(dir: &TempDir, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_human-cli"));
    command
        .args(args)
        .current_dir(dir.path())
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if let Some(stdin) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn json_stdout(output: &Output) -> Value {
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn validate_json_is_headless_and_has_no_status_prefix() {
    let dir = TempDir::new();
    let output = run(&dir, &["--json", "validate", "--seed", "41"], None);
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["valid"], true);
    assert_eq!(value["built"], false);
    assert!(text(&output.stdout).trim_start().starts_with('{'));
}

#[cfg(feature = "editor")]
#[test]
fn editor_binary_dispatches_headless_catalog_without_a_display() {
    let dir = TempDir::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_human-editor"));
    let output = command
        .args(["--headless", "--json", "catalog", "clips"])
        .current_dir(dir.path())
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    let clips: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(clips.as_array().is_some_and(|clips| !clips.is_empty()));
}

#[test]
fn create_reads_stdin_and_project_and_mhm_round_trip() {
    let dir = TempDir::new();
    let project = dir.file("nested/character.project.json");
    let mhm = dir.file("character.mhm");
    let project_arg = project.to_string_lossy().into_owned();
    let mhm_arg = mhm.to_string_lossy().into_owned();

    let created = run(
        &dir,
        &[
            "--json",
            "create",
            "--input",
            "-",
            "--format",
            "project",
            "--output",
            &project_arg,
        ],
        Some(r#"{"name":"Piped Hero","gender":0.2,"age":0.4}"#),
    );
    assert_eq!(json_stdout(&created)["format"], "project");
    assert!(project.is_file());

    let converted = run(
        &dir,
        &[
            "--json",
            "create",
            "--input",
            &project_arg,
            "--format",
            "mhm",
            "--output",
            &mhm_arg,
        ],
        None,
    );
    assert_eq!(json_stdout(&converted)["format"], "mhm");
    assert!(text(&fs::read(&mhm).unwrap()).contains("name Piped Hero"));

    let checked = run(&dir, &["--json", "validate", "--input", &mhm_arg], None);
    assert_eq!(json_stdout(&checked)["name"], "Piped Hero");
}

#[test]
fn static_glb_export_writes_mesh_and_exact_recipe() {
    let dir = TempDir::new();
    let mesh = dir.file("exports/static.glb");
    let recipe = dir.file("exports/static.recipe.json");
    let mesh_arg = mesh.to_string_lossy().into_owned();
    let recipe_arg = recipe.to_string_lossy().into_owned();
    let output = run(
        &dir,
        &[
            "--json",
            "export",
            "--seed",
            "8",
            "--static",
            "--output",
            &mesh_arg,
            "--save-recipe",
            &recipe_arg,
        ],
        None,
    );
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["output"], mesh_arg);
    assert_eq!(&fs::read(&mesh).unwrap()[..4], b"glTF");
    assert!(fs::metadata(&mesh).unwrap().len() > 1024);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(recipe).unwrap()).unwrap()["export"]["mode"],
        "static"
    );
}

#[test]
fn batch_continues_after_failure_and_persists_report() {
    let dir = TempDir::new();
    let manifest = dir.file("batch.json");
    let report = dir.file("reports/result.json");
    let valid_output = "exports/valid.obj";
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "jobs": [
                {"preset": "definitely-not-a-preset", "output": "exports/bad.obj"},
                {"seed": 9, "mode": "static", "output": valid_output}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let manifest_arg = manifest.to_string_lossy().into_owned();
    let report_arg = report.to_string_lossy().into_owned();
    let output = run(
        &dir,
        &[
            "--json",
            "batch",
            "--manifest",
            &manifest_arg,
            "--continue-on-error",
            "--report",
            &report_arg,
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(1));
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["processed"], 2);
    assert_eq!(summary["succeeded"], 1);
    assert_eq!(summary["failed"], 1);
    assert_eq!(
        summary,
        serde_json::from_slice::<Value>(&fs::read(report).unwrap()).unwrap()
    );
    assert!(dir.file(valid_output).is_file());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["ok"], false);
}

#[test]
fn usage_errors_cover_unknown_commands_and_conflicting_sources() {
    let dir = TempDir::new();
    let typo = run(&dir, &["valdiate"], None);
    assert_eq!(typo.status.code(), Some(2));
    assert!(text(&typo.stderr).contains("unrecognized subcommand"));

    let conflict = run(
        &dir,
        &["validate", "--seed", "2", "--preset", "adult female"],
        None,
    );
    assert_eq!(conflict.status.code(), Some(2));
    assert!(text(&conflict.stderr).contains("cannot be used with"));
}

#[test]
fn basic_project_validates_and_exports_obj() {
    let dir = TempDir::new();
    let project = dir.file("basic.json");
    let output = dir.file("basic.obj");
    fs::write(
        &project,
        serde_json::to_vec(&json!({
            "version": 1,
            "name": "Basic Test",
            "weights": {},
            "skin_color": [0.57, 0.36, 0.25]
        }))
        .unwrap(),
    )
    .unwrap();
    let project_arg = project.to_string_lossy().into_owned();
    let output_arg = output.to_string_lossy().into_owned();
    let valid = run(
        &dir,
        &["--json", "basic", "validate", "--input", &project_arg],
        None,
    );
    assert_eq!(json_stdout(&valid)["valid"], true);
    let exported = run(
        &dir,
        &[
            "--json",
            "basic",
            "export",
            "--input",
            &project_arg,
            "--output",
            &output_arg,
        ],
        None,
    );
    assert_eq!(json_stdout(&exported)["output"], output_arg);
    let obj = text(&fs::read(&output).unwrap());
    assert!(obj.contains("\nv "));
    assert!(obj.contains("\nf "));
}

#[test]
fn target_inspect_reports_sparse_target_data_as_json() {
    let dir = TempDir::new();
    let target = dir.file("cheek.target");
    fs::write(&target, "0 0.1 0 0\n17 0 -0.2 0.3\n").unwrap();
    let target_arg = target.to_string_lossy().into_owned();
    let output = run(&dir, &["target", "inspect", &target_arg], None);
    let value = json_stdout(&output);
    assert_eq!(value["vertices"], 2);
    assert_eq!(value["deltas"][0][0], 0);
    assert_eq!(value["deltas"][1][0], 17);
}

#[test]
fn target_inputs_reject_oversize_and_numeric_overflow() {
    let dir = TempDir::new();
    for name in ["huge.target", "huge.target.gz"] {
        let path = dir.file(name);
        fs::File::create(&path)
            .unwrap()
            .set_len(64 * 1024 * 1024 + 1)
            .unwrap();
        let output = run(
            &dir,
            &["--json", "target", "inspect", path.to_str().unwrap()],
            None,
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(text(&output.stderr).contains("limit"));
    }
    let path = dir.file("overflow.target");
    fs::write(&path, "0 1e100 0 0\n").unwrap();
    let output = run(
        &dir,
        &["--json", "target", "inspect", path.to_str().unwrap()],
        None,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("f32"));
}

#[test]
fn ambiguous_custom_targets_are_rejected_before_mutation() {
    let dir = TempDir::new();
    let target = json!({"name":"shape","weight":1.0,"deltas":[[0,[0.1,0.0,0.0]]]});
    for targets in [
        json!([target.clone(), target.clone()]),
        json!([{"name":"","weight":1.0,"deltas":[]}]),
        json!([{"name":"shape","weight":1.0,"deltas":[[0,[0.1,0.0,0.0]],[0,[0.2,0.0,0.0]]]}]),
    ] {
        let mut recipe = serde_json::to_value(human_editor::ara::AraRecipe::default()).unwrap();
        recipe["custom_targets"] = targets;
        let output = run(
            &dir,
            &["--json", "create", "--input", "-", "--output", "bad.json"],
            Some(&recipe.to_string()),
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(!dir.file("bad.json").exists());
    }
}

#[cfg(unix)]
#[test]
fn export_rejects_symlink_aliases_for_mesh_and_recipe() {
    let dir = TempDir::new();
    fs::create_dir(dir.file("real")).unwrap();
    std::os::unix::fs::symlink(dir.file("real"), dir.file("alias")).unwrap();
    let output = run(
        &dir,
        &[
            "--json",
            "export",
            "--mode",
            "static",
            "--output",
            "real/a.glb",
            "--save-recipe",
            "alias/a.glb",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("different paths"));
    assert!(!dir.file("real/a.glb").exists());
}

#[test]
fn legacy_list_validates_flags_and_accepts_global_options() {
    let dir = TempDir::new();
    let output = Command::new(env!("CARGO_BIN_EXE_human-export"))
        .args(["--list", "--typo"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let output = Command::new(env!("CARGO_BIN_EXE_human-export"))
        .args(["--json", "--list"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(json_stdout(&output)["presets"].is_array());
    let output = Command::new(env!("CARGO_BIN_EXE_human-export"))
        .args(["--json", "catalog", "clips"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(json_stdout(&output).is_array());
}

#[test]
fn basic_create_exposes_named_controls_and_preserves_project_format() {
    let dir = TempDir::new();
    let value = json_stdout(&run(
        &dir,
        &[
            "basic",
            "create",
            "--name",
            "Basic NPC",
            "--weight",
            "height=0.4",
            "--skin",
            "#8b6f57",
        ],
        None,
    ));
    assert_eq!(value["version"], 1);
    assert_eq!(value["name"], "Basic NPC");
    assert!((value["weights"]["height"].as_f64().unwrap() - 0.4).abs() < 1e-6);
    let catalog = json_stdout(&run(&dir, &["basic", "catalog"], None));
    assert!(
        catalog
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "height")
    );
}

#[test]
fn cartoon_survives_wardrobe_application_and_rejects_invalid_strength() {
    let dir = TempDir::new();
    let saved = run(
        &dir,
        &[
            "wardrobe",
            "create",
            "--wardrobe-name",
            "Everyday",
            "--set",
            "top=tank_top",
            "--output",
            "outfit.json",
        ],
        None,
    );
    assert!(saved.status.success(), "{}", text(&saved.stderr));
    let recipe = json_stdout(&run(
        &dir,
        &[
            "wardrobe",
            "apply",
            "outfit.json",
            "--set",
            "cartoon.enabled=true",
            "--set",
            "cartoon.strength=0.75",
            "--set",
            "top=none",
        ],
        None,
    ));
    assert_eq!(recipe["cartoon"]["enabled"], true);
    assert_eq!(recipe["cartoon"]["strength"], 0.75);
    assert_eq!(
        recipe["params"]["top"], "none",
        "explicit overrides follow wardrobe application"
    );
    let invalid = run(&dir, &["create", "--set", "cartoon.strength=1.5"], None);
    assert_eq!(invalid.status.code(), Some(1));
}

#[cfg(unix)]
#[test]
fn failed_garment_output_does_not_write_shared_texture_and_hair_import_works() {
    let dir = TempDir::new();
    fs::create_dir(dir.file("assets")).unwrap();
    let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/native");
    for file in ["catalog.json", "base.pack", "targets.pack"] {
        std::os::unix::fs::symlink(native.join(file), dir.file("assets").join(file)).unwrap();
    }
    fs::write(
        dir.file("hair.obj"),
        "v 0 1 0\nv .1 1 0\nv 0 1 .1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n",
    )
    .unwrap();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]))
        .save(dir.file("white.png"))
        .unwrap();
    fs::write(dir.file("blocked"), "not a directory").unwrap();
    let failed = run(
        &dir,
        &[
            "--assets",
            "assets",
            "wardrobe",
            "import",
            "hair.obj",
            "--id",
            "test_hair",
            "--slot",
            "hair",
            "--texture",
            "white.png",
            "--output",
            "blocked/garment.json",
        ],
        None,
    );
    assert_eq!(failed.status.code(), Some(1));
    assert!(!dir.file("assets/textures/custom_test_hair.png").exists());
    let success = run(
        &dir,
        &[
            "--assets",
            "assets",
            "wardrobe",
            "import",
            "hair.obj",
            "--id",
            "test_hair",
            "--slot",
            "hair",
            "--output",
            "hair.json",
        ],
        None,
    );
    assert!(success.status.success(), "{}", text(&success.stderr));
    let asset: Value = serde_json::from_slice(&fs::read(dir.file("hair.json")).unwrap()).unwrap();
    assert_eq!(asset["slot"], "hair");
}
