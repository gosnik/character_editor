//! Headless authoring commands shared by both command-line binaries.
use crate::{
    ara::{AraRecipe, CustomTarget, native_catalog, native_catalog_with_assets},
    cli_io, mpfb,
    native_assets::NativeLibrary,
    native_character::NativeCharacter,
    native_export::export_character,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    name = "human-cli",
    version,
    propagate_version = true,
    about = "Headless Human Studio character authoring",
    long_about = "Create, customize, validate and export MakeHuman-derived characters without a window, display, GPU, or authoring runtime. All controls use the editor's native Rust engine."
)]
struct Cli {
    /// Native asset library (also HUMAN_EDITOR_ASSETS)
    #[arg(long,global=true,env="HUMAN_EDITOR_ASSETS",default_value=concat!(env!("CARGO_MANIFEST_DIR"),"/assets/native"))]
    assets: PathBuf,
    /// Machine-readable results and runtime errors
    #[arg(long, global = true)]
    json: bool,
    /// Suppress status messages; recipe/catalog output is retained
    #[arg(short, long, global = true)]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// List presets, clips, fields, detail sliders, garments, or asset metadata
    Catalog {
        #[arg(default_value="presets",value_parser=["all","presets","clips","fields","sliders","garments","outfit_sets","defaults","assets"])]
        section: String,
        /// Case-insensitive search in entries (including nested slider groups)
        #[arg(long)]
        filter: Option<String>,
    },
    /// Print complete control metadata, breast ranges, and native recipe defaults
    Schema,
    /// Create/edit a recipe, or convert JSON projects and MakeHuman MHM files
    #[command(alias = "edit", alias = "convert")]
    Create {
        #[command(flatten)]
        source: Source,
        /// Destination file, or - for standard output
        #[arg(short, long, alias = "out", default_value = "-")]
        output: PathBuf,
        #[arg(long, value_enum, default_value = "native")]
        format: RecipeFormat,
    },
    /// Validate parameters; optionally construct geometry and validate asset references
    Validate {
        #[command(flatten)]
        source: Source,
        #[arg(long)]
        build: bool,
    },
    /// Inspect geometry, materials, bounds, skeleton and animation availability
    Inspect {
        #[command(flatten)]
        source: Source,
        /// Show the normalized editable recipe without constructing geometry
        #[arg(long)]
        recipe_only: bool,
    },
    /// Export a character as textured GLB or OBJ
    Export {
        #[command(flatten)]
        source: Source,
        #[command(flatten)]
        options: ExportArgs,
    },
    /// Run a JSON manifest of exports, sharing the asset library between jobs
    Batch {
        #[arg(short, long)]
        manifest: PathBuf,
        #[arg(long)]
        continue_on_error: bool,
        /// Write a machine-readable result report (also printed with --json)
        #[arg(long)]
        report: Option<PathBuf>,
    },
    /// Import a supported external animation library
    AnimationImport {
        #[arg(long, value_parser=["ual1", "ual2", "meshy-female", "meshy-male"])]
        id: String,
        /// Library directory, Unreal-Godot directory, or GLB file
        #[arg(long)]
        source: PathBuf,
    },
    /// Inspect or mirror MakeHuman custom target files
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// Create outfit libraries, import garments, and test wardrobes across bodies/poses
    Wardrobe {
        #[command(subcommand)]
        command: WardrobeCommand,
    },
    /// Create, customize, validate, inspect or export original Basic editor projects
    Basic {
        #[arg(value_parser=["catalog","create","validate","inspect","export"])]
        action: String,
        #[arg(short, long)]
        input: Option<PathBuf>,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_name = "ID=WEIGHT")]
        weight: Vec<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_name = "#RRGGBB")]
        skin: Option<String>,
        #[arg(long,default_value=concat!(env!("CARGO_MANIFEST_DIR"),"/assets/makehuman"))]
        library: PathBuf,
    },
}
#[derive(Clone, Default, Args)]
#[group(multiple = true)]
struct Source {
    /// Project JSON, native recipe, reference params JSON, or .mhm; - reads JSON stdin
    #[arg(short,long,alias="params",conflicts_with_all=["preset","seed"])]
    input: Option<PathBuf>,
    /// Complete preset name; discover with catalog presets
    #[arg(long, conflicts_with = "seed")]
    preset: Option<String>,
    /// Generate a reproducible random character
    #[arg(long)]
    seed: Option<u64>,
    /// Repeatable NAME=VALUE or /JSON/pointer=VALUE; accepts JSON or a string
    #[arg(long = "set", value_name = "NAME=VALUE")]
    sets: Vec<String>,
    /// Repeatable detail morph ID=WEIGHT; discover with catalog sliders
    #[arg(long, value_name = "ID=WEIGHT")]
    modifier: Vec<String>,
    /// Repeatable GARMENT.FIELD=VALUE (color, color2, pattern)
    #[arg(long, value_name = "GARMENT.FIELD=VALUE")]
    style: Vec<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long,value_parser=["low","medium","high"])]
    detail: Option<String>,
    #[arg(long,value_parser=["none","game","game_lite","full"])]
    rig: Option<String>,
    #[arg(long,value_parser=["A","T"])]
    rest_pose: Option<String>,
    /// Apply a saved outfit before parameter overrides; repeat to layer custom assets
    #[arg(long)]
    wardrobe: Vec<PathBuf>,
    /// Add/replace a portable custom garment JSON asset by its ID
    #[arg(long)]
    garment: Vec<PathBuf>,
    #[arg(long, value_name = "ID")]
    remove_garment: Vec<String>,
    /// Import .target/.target.gz; may be repeated
    #[arg(long, value_name = "FILE")]
    target: Vec<PathBuf>,
    /// Set imported/existing target weight by NAME=WEIGHT
    #[arg(long, value_name = "NAME=WEIGHT")]
    target_weight: Vec<String>,
    /// Append an X-mirrored copy of an imported/existing target
    #[arg(long, value_name = "NAME")]
    mirror_target: Vec<String>,
    #[arg(long, value_name = "NAME")]
    remove_target: Vec<String>,
}
#[derive(Clone, Copy, ValueEnum)]
enum RecipeFormat {
    Native,
    Project,
    Mhm,
}
impl RecipeFormat {
    fn id(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Project => "project",
            Self::Mhm => "mhm",
        }
    }
}
#[derive(Args)]
struct ExportArgs {
    #[arg(short, long, alias = "out", default_value = "exports/character.glb")]
    output: PathBuf,
    /// Infer from the file extension unless supplied
    #[arg(long,value_parser=["glb","obj"])]
    format: Option<String>,
    #[arg(long,value_parser=["static","rigged","animated"],conflicts_with="static_mesh")]
    mode: Option<String>,
    /// Compatibility shorthand for --mode static
    #[arg(long = "static")]
    static_mesh: bool,
    /// Repeat, or comma-separate, animation IDs
    #[arg(long, value_delimiter = ',', conflicts_with = "all_clips")]
    clip: Vec<String>,
    #[arg(long)]
    all_clips: bool,
    /// Include root translation; optionally pass true/false
    #[arg(long,num_args=0..=1,default_missing_value="true")]
    root_motion: Option<bool>,
    /// Save the exact native recipe used for this export
    #[arg(long)]
    save_recipe: Option<PathBuf>,
}
#[derive(Subcommand)]
enum TargetCommand {
    Inspect {
        input: PathBuf,
    },
    Mirror {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Maximum reflected-vertex matching distance in MakeHuman decimeters
        #[arg(long, default_value = "0.01")]
        tolerance: f32,
    },
}
#[derive(Subcommand)]
enum WardrobeCommand {
    /// Save a reusable outfit, including optional imported garment files
    Create {
        #[command(flatten)]
        source: Source,
        /// Profile name; source character naming continues to use --name.
        #[arg(long = "wardrobe-name")]
        wardrobe_name: String,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        include_hair: bool,
        /// Capture facial hair independently of scalp hair.
        #[arg(long)]
        include_beard: bool,
    },
    /// Apply a saved wardrobe to a character without changing body morphology
    Apply {
        profile: PathBuf,
        #[command(flatten)]
        source: Source,
        #[arg(short, long, default_value = "-")]
        output: PathBuf,
        #[arg(long, value_enum, default_value = "native")]
        format: RecipeFormat,
    },
    /// Inspect a saved outfit and its portable garment inventory
    Inspect { profile: PathBuf },
    /// Export the exact neutral authoring body, hip-origin/Y-up, for garment modeling
    Template {
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long,value_parser=["m","dm"],default_value="m")]
        units: String,
    },
    /// Import a UV-mapped OBJ garment authored against the wardrobe template
    Import {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long,value_parser=["hair","beard","top","bottom","jacket","feet","hat","accessory"])]
        slot: String,
        #[arg(long,value_parser=["m","dm"],default_value="m")]
        units: String,
        /// Bind each nearby garment vertex to a local native body triangle.
        /// Keeps source geometry and UVs unchanged while avoiding cross-surface anchors.
        #[arg(long, visible_alias = "surface-binding")]
        surface_bind: bool,
        #[arg(long, default_value = "#ffffff")]
        color: String,
        /// Optional image copied into the selected native asset library
        #[arg(long)]
        texture: Option<PathBuf>,
        /// Optional JSON array of original body quad indices covered by this garment
        #[arg(long)]
        hide_faces: Option<PathBuf>,
    },
    /// Report fit and skinned geometry across body presets and sampled poses
    Test {
        profile: PathBuf,
        #[arg(long)]
        input: Vec<PathBuf>,
        #[arg(long)]
        preset: Vec<String>,
        #[arg(long, value_delimiter = ',', default_value = "idle,walk")]
        clip: Vec<String>,
        #[arg(long, default_value_t = 3)]
        samples: usize,
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        export_dir: Option<PathBuf>,
        /// Include up to 256 exact contacting triangles per pose for this part (for example custom:coat)
        #[arg(long)]
        contact_details: Option<String>,
        /// Return failure if any fit warning is reported
        #[arg(long)]
        strict: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    jobs: Vec<BatchJob>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchJob {
    input: Option<PathBuf>,
    preset: Option<String>,
    seed: Option<u64>,
    #[serde(default)]
    set: Vec<String>,
    #[serde(default)]
    wardrobes: Vec<PathBuf>,
    output: PathBuf,
    clips: Option<Vec<String>>,
    mode: Option<String>,
    root_motion: Option<bool>,
    save_recipe: Option<PathBuf>,
}

/// Process exit status: 0 success/help, 1 operation/validation failure, 2 usage error.
pub fn entry(args: impl IntoIterator<Item = OsString>) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    match run(&cli) {
        Ok(()) => 0,
        Err(error) => {
            if cli.json {
                eprintln!("{}", json!({"ok":false,"error":error}));
            } else {
                eprintln!("error: {error}");
            }
            1
        }
    }
}
/// Keep the original human-export flags working while offering the full command set.
pub fn legacy_entry(args: impl IntoIterator<Item = OsString>) -> i32 {
    let mut args: Vec<_> = args.into_iter().collect();
    if args.is_empty() {
        args.push(OsString::from("human-export"));
    }
    let mut index = 1;
    while let Some(flag) = args.get(index).and_then(|a| a.to_str()) {
        if flag == "--assets" {
            index += 2;
        } else if matches!(flag, "--json" | "--quiet" | "-q") || flag.starts_with("--assets=") {
            index += 1;
        } else {
            break;
        }
    }
    let first = args.get(index).and_then(|a| a.to_str()).unwrap_or("");
    if ![
        "catalog",
        "schema",
        "create",
        "edit",
        "convert",
        "validate",
        "inspect",
        "export",
        "batch",
        "target",
        "basic",
        "wardrobe",
        "animation-import",
    ]
    .contains(&first)
    {
        if args.iter().any(|a| a == "--list") {
            args.retain(|a| a != "--list");
            args.insert(1, OsString::from("catalog"));
            args.insert(2, OsString::from("all"));
        } else {
            args.insert(1, OsString::from("export"));
            if !args.iter().any(|a| {
                a == "--clip" || a == "--all-clips" || a.to_string_lossy().starts_with("--clip=")
            }) {
                args.extend([OsString::from("--clip"), OsString::from("idle")]);
            }
        }
    }
    entry(args)
}
fn emit(value: &Value) -> Result<(), String> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer_pretty(&mut out, value).map_err(|e| e.to_string())?;
    writeln!(out).map_err(|e| e.to_string())
}
fn status(cli: &Cli, value: Value, text: &str) -> Result<(), String> {
    if cli.quiet {
        Ok(())
    } else if cli.json {
        emit(&value)
    } else {
        println!("{text}");
        Ok(())
    }
}
fn split_assignment(value: &str) -> Result<(&str, &str), String> {
    value
        .split_once('=')
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .ok_or_else(|| format!("expected NAME=VALUE: {value}"))
}
fn source_recipe(source: &Source, assets: &Path) -> Result<AraRecipe, String> {
    if usize::from(source.input.is_some())
        + usize::from(source.preset.is_some())
        + usize::from(source.seed.is_some())
        > 1
    {
        return Err("input, preset and seed are mutually exclusive".into());
    }
    let mut recipe = if let Some(path) = &source.input {
        cli_io::load(path, assets)?
    } else if let Some(name) = &source.preset {
        let catalog = native_catalog();
        let preset = catalog["presets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == *name)
            .ok_or_else(|| format!("unknown preset {name:?}; use catalog presets"))?;
        AraRecipe::from_reference(preset["params"].clone())?
    } else if let Some(seed) = source.seed {
        AraRecipe::random(seed)
    } else {
        AraRecipe::default()
    };
    for path in &source.wardrobe {
        crate::wardrobe::WardrobeProfile::load(path)?.apply(&mut recipe)?;
    }
    for path in &source.garment {
        let garment: crate::wardrobe_assets::CustomGarment =
            serde_json::from_slice(&read_bounded(path)?).map_err(|e| format!("garment: {e}"))?;
        garment.validate()?;
        if let Some(old) = recipe
            .custom_garments
            .iter_mut()
            .find(|g| g.id == garment.id)
        {
            *old = garment;
        } else {
            recipe.custom_garments.push(garment);
        }
    }
    for id in &source.remove_garment {
        let index = recipe
            .custom_garments
            .iter()
            .position(|g| g.id == *id)
            .ok_or_else(|| format!("unknown custom garment {id:?}"))?;
        recipe.custom_garments.remove(index);
    }
    // Newly-created CLI recipes export one clip by default; loaded recipes keep their export selection.
    if source.input.is_none() {
        recipe.export.clips = vec![recipe.preview_clip.clone()];
    }
    for assignment in &source.sets {
        cli_io::set(&mut recipe, assignment)?;
    }
    for assignment in &source.modifier {
        let (id, value) = split_assignment(assignment)?;
        let escaped = id.replace('~', "~0").replace('/', "~1");
        cli_io::set(&mut recipe, &format!("/params/modifiers/{escaped}={value}"))?;
    }
    for assignment in &source.style {
        cli_io::set(&mut recipe, &format!("garmentStyles.{assignment}"))?;
    }
    for (key, value) in [
        ("name", source.name.as_ref()),
        ("detail", source.detail.as_ref()),
        ("rig", source.rig.as_ref()),
        ("restPose", source.rest_pose.as_ref()),
    ] {
        if let Some(value) = value {
            recipe.params[key] = json!(value);
        }
    }
    for name in &source.remove_target {
        let index = recipe
            .custom_targets
            .iter()
            .position(|t| t.name == *name)
            .ok_or_else(|| format!("unknown custom target {name:?}"))?;
        recipe.custom_targets.remove(index);
    }
    for path in &source.target {
        let name = path
            .file_name()
            .ok_or("target path has no filename")?
            .to_string_lossy()
            .into_owned();
        if recipe.custom_targets.iter().any(|t| t.name == name) {
            return Err(format!("duplicate target name {name:?}"));
        }
        recipe.custom_targets.push(CustomTarget {
            name,
            weight: 1.,
            deltas: mpfb::load_target(path, 19158)?,
        });
    }
    for assignment in &source.target_weight {
        let (name, value) = split_assignment(assignment)?;
        let target = recipe
            .custom_targets
            .iter_mut()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("unknown custom target {name:?}"))?;
        target.weight = value
            .parse()
            .map_err(|_| format!("invalid target weight: {value}"))?;
    }
    if !source.mirror_target.is_empty() {
        let library = NativeLibrary::load(assets)?;
        let coords = base_positions(&library)?;
        for name in &source.mirror_target {
            let target = recipe
                .custom_targets
                .iter()
                .find(|t| t.name == *name)
                .ok_or_else(|| format!("unknown custom target {name:?}"))?
                .clone();
            let mirrored_name = format!("{} mirrored", target.name);
            if recipe
                .custom_targets
                .iter()
                .any(|t| t.name == mirrored_name)
            {
                return Err(format!("duplicate target name {mirrored_name:?}"));
            }
            recipe.custom_targets.push(CustomTarget {
                name: mirrored_name,
                weight: target.weight,
                deltas: mpfb::mirror_target(&target.deltas, &coords, 0.01)?,
            });
        }
    }
    cli_io::validate(&recipe)?;
    Ok(recipe)
}
fn base_positions(lib: &NativeLibrary) -> Result<Vec<[f32; 3]>, String> {
    Ok(lib
        .base
        .arrays
        .get("positions")
        .ok_or("missing base positions")?
        .f32()?
        .chunks_exact(3)
        .map(|p| [p[0], p[1], p[2]])
        .collect())
}
fn prepare_output(path: &Path) -> Result<(), String> {
    if path == Path::new("-") {
        return Err("mesh/binary output requires a file path".into());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    Ok(())
}
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    prepare_output(path)?;
    crate::model::atomic_write(path, bytes).map_err(|e| e.to_string())
}
fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let reader: Box<dyn Read> = if path == Path::new("-") {
        Box::new(io::stdin())
    } else {
        Box::new(fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?)
    };
    let mut data = Vec::new();
    reader
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > 64 * 1024 * 1024 {
        return Err("input exceeds 64 MiB".into());
    }
    Ok(data)
}
fn configure_export(
    recipe: &mut AraRecipe,
    options: &ExportArgs,
    assets: &Path,
) -> Result<(), String> {
    let extension = options
        .output
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    let format = options
        .format
        .as_deref()
        .or(extension.as_deref())
        .ok_or("output needs .glb/.obj extension or --format")?;
    if !["glb", "obj"].contains(&format) {
        return Err(format!("unsupported export format {format:?}"));
    }
    if extension.as_deref().is_some_and(|e| e != format) {
        return Err("output extension conflicts with --format".into());
    }
    recipe.export.format = format.into();
    if let Some(mode) = &options.mode {
        recipe.export.mode = mode.clone();
    }
    if options.static_mesh {
        recipe.export.mode = "static".into();
    }
    if options.all_clips {
        recipe.export.clips = native_catalog_with_assets(assets)["clips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect();
    } else if !options.clip.is_empty() {
        recipe.export.clips = options.clip.clone();
        recipe.preview_clip = options.clip[0].clone();
    }
    if let Some(root_motion) = options.root_motion {
        recipe.export.root_motion = root_motion;
    }
    cli_io::validate(recipe)
}
fn inspect(character: &NativeCharacter) -> Value {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for part in &character.parts {
        for p in &part.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
    }
    json!({"name":character.recipe.params["name"],"height_m":character.height,"triangles":character.triangle_count(),"vertices":character.parts.iter().map(|p|p.positions.len()).sum::<usize>(),"bounds_m":{"min":min,"max":max},
        "hair_fit":{"corrected_vertices":character.hair_fit.corrected_vertices,"max_displacement_m":character.hair_fit.max_displacement*0.1,"remaining_clearance_warnings":character.hair_fit.remaining_violations},
        "parts":character.parts.iter().map(|p|json!({"name":p.name,"vertices":p.positions.len(),"triangles":p.indices.len()/3,"rigid_joint":p.rigid_joint,"material":p.material})).collect::<Vec<_>>(),
        "rig":character.rig.as_ref().map(|r|json!({"kind":r.kind,"rest_pose":r.rest_pose,"bones":r.bones.iter().map(|b|json!({"name":b.name,"parent":b.parent,"head_m":b.head.to_array(),"tail_m":b.tail.to_array()})).collect::<Vec<_>>(),"clips":r.clip_ids().into_iter().map(|id|{json!({"id":id,"duration_s":r.clip_duration(&id),"looped":r.clip_looped(&id)})}).collect::<Vec<_>>()}))})
}
fn perform_export(
    lib: &NativeLibrary,
    recipe: &AraRecipe,
    options: &ExportArgs,
) -> Result<Value, String> {
    if options.save_recipe.as_deref() == Some(Path::new("-")) {
        return Err("--save-recipe requires a file path; use create for stdout recipes".into());
    }
    if let Some(path) = &options.save_recipe {
        if normalized(path)? == normalized(&options.output)? {
            return Err("mesh output and saved recipe must have different paths".into());
        }
    }
    let started = std::time::Instant::now();
    let character = NativeCharacter::build(lib, recipe)?;
    prepare_output(&options.output)?;
    export_character(&character, recipe, &lib.root, &options.output)?;
    if let Some(path) = &options.save_recipe {
        cli_io::save(recipe, path, "native")?;
    }
    Ok(
        json!({"ok":true,"name":recipe.params["name"],"output":options.output,"height_m":character.height,"triangles":character.triangle_count(),"parts":character.parts.len(),"bones":character.rig.as_ref().map_or(0,|r|r.bones.len()),"clips":if character.rig.is_some() && recipe.params["animations"]!=false && recipe.export.mode=="animated"&&recipe.export.format=="glb" {recipe.export.clips.clone()}else{vec![]},"elapsed_s":started.elapsed().as_secs_f64()}),
    )
}
fn run(cli: &Cli) -> Result<(), String> {
    match &cli.command {
        Command::AnimationImport { id, source } => {
            let catalog =
                crate::native_animation::import_library(&crate::native_animation::ImportOptions {
                    id: id.clone(),
                    source: source.clone(),
                    output_root: cli.assets.clone(),
                })?;
            status(
                cli,
                json!({"ok":true,"clips":catalog.clips}),
                "Animation library imported",
            )
        }
        Command::Catalog { section, filter } => {
            let schema = native_catalog_with_assets(&cli.assets);
            let mut value = if section == "assets" {
                serde_json::from_slice(
                    &fs::read(cli.assets.join("catalog.json")).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
            } else if section == "all" {
                schema
            } else {
                schema
                    .get(section)
                    .cloned()
                    .ok_or_else(|| format!("catalog section {section:?} is unavailable"))?
            };
            if let Some(filter) = filter {
                let query = filter.to_lowercase();
                if let Some(entries) = value.as_array_mut() {
                    entries.retain(|v| v.to_string().to_lowercase().contains(&query));
                } else if let Some(entries) = value.as_object_mut() {
                    entries.retain(|k, v| {
                        k.to_lowercase().contains(&query)
                            || v.to_string().to_lowercase().contains(&query)
                    });
                }
            }
            emit(&value)
        }
        Command::Schema => emit(
            &json!({"catalog":native_catalog_with_assets(&cli.assets),"recipe_defaults":AraRecipe::default(),"cartoon_controls":[{"id":"enabled","type":"boolean","default":false},{"id":"strength","min":0.,"max":1.,"default":1.}],"breast_controls":[
            {"id":"volume","min":0.,"max":2.5,"default":1.},{"id":"projection","min":0.2,"max":2.5,"default":1.},
            {"id":"lift","min":-1.,"max":1.,"default":0.},{"id":"separation","min":-0.5,"max":0.5,"default":0.},
            {"id":"dynamics","type":"boolean","default":true},{"id":"stiffness","min":2.,"max":80.,"default":22.},
            {"id":"damping","min":0.1,"max":2.,"default":0.65},{"id":"motion_strength","min":0.,"max":2.,"default":0.65}]}),
        ),
        Command::Create {
            source,
            output,
            format,
        } => {
            let recipe = source_recipe(source, &cli.assets)?;
            cli_io::save(&recipe, output, format.id())?;
            if output != Path::new("-") {
                status(
                    cli,
                    json!({"ok":true,"output":output,"format":format.id()}),
                    &format!("Saved {}", output.display()),
                )?;
            }
            Ok(())
        }
        Command::Validate { source, build } => {
            let recipe = source_recipe(source, &cli.assets)?;
            let mut value =
                json!({"ok":true,"valid":true,"built":build,"name":recipe.params["name"]});
            if *build {
                let lib = NativeLibrary::load(&cli.assets)?;
                value["character"] = inspect(&NativeCharacter::build(&lib, &recipe)?);
            }
            status(cli, value, "Character is valid")
        }
        Command::Inspect {
            source,
            recipe_only,
        } => {
            let recipe = source_recipe(source, &cli.assets)?;
            if *recipe_only {
                emit(&serde_json::to_value(recipe).map_err(|e| e.to_string())?)
            } else {
                let lib = NativeLibrary::load(&cli.assets)?;
                emit(&inspect(&NativeCharacter::build(&lib, &recipe)?))
            }
        }
        Command::Export { source, options } => {
            let mut recipe = source_recipe(source, &cli.assets)?;
            configure_export(&mut recipe, options, &cli.assets)?;
            let library = NativeLibrary::load(&cli.assets)?;
            let result = perform_export(&library, &recipe, options)?;
            status(
                cli,
                result,
                &format!("Exported {}", options.output.display()),
            )
        }
        Command::Wardrobe { command } => wardrobe_command(cli, command),
        Command::Batch {
            manifest,
            continue_on_error,
            report,
        } => batch(cli, manifest, *continue_on_error, report.as_deref()),
        Command::Target { command } => match command {
            TargetCommand::Inspect { input } => {
                let deltas = mpfb::load_target(input, 19158)?;
                emit(&json!({"input":input,"vertices":deltas.len(),"deltas":deltas}))
            }
            TargetCommand::Mirror {
                input,
                output,
                tolerance,
            } => {
                let deltas = mpfb::load_target(input, 19158)?;
                let lib = NativeLibrary::load(&cli.assets)?;
                let deltas = mpfb::mirror_target(&deltas, &base_positions(&lib)?, *tolerance)?;
                let text = deltas
                    .iter()
                    .map(|(i, d)| format!("{i} {} {} {}\n", d[0], d[1], d[2]))
                    .collect::<String>();
                let bytes = if output.extension().is_some_and(|e| e == "gz") {
                    let mut writer =
                        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    writer
                        .write_all(text.as_bytes())
                        .map_err(|e| e.to_string())?;
                    writer.finish().map_err(|e| e.to_string())?
                } else {
                    text.into_bytes()
                };
                write_bytes(output, &bytes)?;
                status(
                    cli,
                    json!({"ok":true,"output":output,"vertices":deltas.len()}),
                    &format!("Saved {}", output.display()),
                )
            }
        },
        Command::Basic {
            action,
            input,
            output,
            library,
            weight,
            name,
            skin,
        } => {
            let lib = crate::CharacterLibrary::load(library).map_err(|e| e.to_string())?;
            if action == "catalog" {
                return emit(&json!(lib.targets.iter().map(|t| json!({"id":t.definition.id,"label":t.definition.label,"category":t.definition.category,"min":-1.,"max":1.})).collect::<Vec<_>>()));
            }
            let mut recipe = if let Some(input) = input {
                serde_json::from_slice::<crate::CharacterRecipe>(&read_bounded(input)?)
                    .map_err(|e| e.to_string())?
            } else if action == "create" {
                crate::CharacterRecipe::default()
            } else {
                return Err("basic validate/inspect/export requires --input".into());
            };
            for assignment in weight {
                let (id, value) = split_assignment(assignment)?;
                recipe.weights.insert(
                    id.into(),
                    value
                        .parse()
                        .map_err(|_| format!("invalid weight {value:?}"))?,
                );
            }
            if let Some(name) = name {
                recipe.name = name.clone();
            }
            if let Some(color) = skin {
                if !crate::ara::valid_color(color) {
                    return Err("skin must be #RRGGBB".into());
                }
                recipe.skin_color = crate::ara::rgb(color);
            }
            lib.validate(&recipe).map_err(|e| e.to_string())?;
            if recipe.customization.is_some() {
                return Err("basic command requires a Basic editor project; use the native commands for this project".into());
            }
            match action.as_str() {
                "create" => {
                    let path = output.as_deref().unwrap_or(Path::new("-"));
                    if path == Path::new("-") {
                        emit(&serde_json::to_value(recipe).map_err(|e| e.to_string())?)
                    } else {
                        write_bytes(
                            path,
                            &serde_json::to_vec_pretty(&recipe).map_err(|e| e.to_string())?,
                        )?;
                        status(
                            cli,
                            json!({"ok":true,"output":path}),
                            &format!("Saved {}", path.display()),
                        )
                    }
                }
                "export" => {
                    let path = output
                        .as_ref()
                        .ok_or("basic export requires --output FILE.obj")?;
                    if path.extension().is_none_or(|e| e != "obj") {
                        return Err("basic export supports .obj files".into());
                    }
                    prepare_output(path)?;
                    lib.export_obj(&recipe, path).map_err(|e| e.to_string())?;
                    status(
                        cli,
                        json!({"ok":true,"output":path,"engine":"basic","has_native_draft":recipe.native_draft.is_some()}),
                        &format!("Exported {}", path.display()),
                    )
                }
                "inspect" => emit(
                    &json!({"engine":"basic","has_native_draft":recipe.native_draft.is_some(),"recipe":recipe,"vertices":lib.base_positions().len(),"triangles":lib.triangles().len()/3}),
                ),
                _ => status(
                    cli,
                    json!({"ok":true,"valid":true,"engine":"basic","has_native_draft":recipe.native_draft.is_some()}),
                    "Basic project is valid",
                ),
            }
        }
    }
}
fn batch(cli: &Cli, path: &Path, keep_going: bool, report: Option<&Path>) -> Result<(), String> {
    let manifest: Manifest =
        serde_json::from_slice(&read_bounded(path)?).map_err(|e| format!("batch manifest: {e}"))?;
    if manifest.version != 1 || manifest.jobs.is_empty() || manifest.jobs.len() > 10000 {
        return Err("manifest requires version 1 and 1..10000 jobs".into());
    }
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let relative = |p: &Path| {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        }
    };
    let mut outputs = std::collections::BTreeSet::new();
    for artifact in manifest
        .jobs
        .iter()
        .flat_map(|job| std::iter::once(&job.output).chain(job.save_recipe.iter()))
    {
        if artifact == Path::new("-") {
            return Err("batch artifacts require file paths".into());
        }
        if !outputs.insert(normalized(&relative(artifact))?) {
            return Err("batch mesh and recipe outputs must have distinct paths".into());
        }
    }
    if let Some(report) = report {
        if report == Path::new("-") || !outputs.insert(normalized(report)?) {
            return Err("batch report must have a distinct file path".into());
        }
    }
    if manifest
        .jobs
        .iter()
        .any(|j| j.input.as_deref() == Some(Path::new("-")))
    {
        return Err("batch job inputs must be files, not stdin".into());
    }
    let library = NativeLibrary::load(&cli.assets)?;
    let mut results = vec![];
    let mut failed = 0;
    for (index, job) in manifest.jobs.iter().enumerate() {
        let source = Source {
            input: job.input.as_ref().map(|p| relative(p)),
            preset: job.preset.clone(),
            seed: job.seed,
            sets: job.set.clone(),
            wardrobe: job.wardrobes.iter().map(|p| relative(p)).collect(),
            ..Default::default()
        };
        let options = ExportArgs {
            output: relative(&job.output),
            format: None,
            mode: job.mode.clone(),
            static_mesh: false,
            clip: job.clips.clone().unwrap_or_default(),
            all_clips: false,
            root_motion: job.root_motion,
            save_recipe: job.save_recipe.as_ref().map(|p| relative(p)),
        };
        let result = (|| {
            let mut recipe = source_recipe(&source, &cli.assets)?;
            if let Some(clips) = &job.clips {
                recipe.export.clips = clips.clone();
            }
            configure_export(&mut recipe, &options, &cli.assets)?;
            perform_export(&library, &recipe, &options)
        })();
        match result {
            Ok(value) => results.push(json!({"index":index,"result":value})),
            Err(error) => {
                failed += 1;
                results
                    .push(json!({"index":index,"ok":false,"output":options.output,"error":error}));
                if !keep_going {
                    break;
                }
            }
        }
    }
    let summary = json!({"ok":failed==0,"total":manifest.jobs.len(),"processed":results.len(),"succeeded":results.len()-failed,"failed":failed,"jobs":results});
    if let Some(report) = report {
        write_bytes(
            report,
            &serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?,
        )?;
    }
    status(
        cli,
        summary,
        &format!(
            "Batch: {} succeeded, {failed} failed",
            results.len() - failed
        ),
    )?;
    if failed > 0 {
        Err(format!("{failed} batch job(s) failed"))
    } else {
        Ok(())
    }
}

fn normalized(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                result.push(name);
                // Resolve existing directory aliases before processing '..' or
                // adding non-existing output names, without creating artifacts.
                match fs::canonicalize(&result) {
                    Ok(real) => result = real,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("{}: {error}", result.display())),
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

fn wardrobe_command(cli: &Cli, command: &WardrobeCommand) -> Result<(), String> {
    use crate::wardrobe::WardrobeProfile;
    match command {
        WardrobeCommand::Create {
            source,
            wardrobe_name,
            output,
            include_hair,
            include_beard,
        } => {
            let recipe = source_recipe(source, &cli.assets)?;
            let profile = WardrobeProfile::capture_with_beard(
                &recipe,
                wardrobe_name,
                *include_hair,
                *include_beard,
            )?;
            if output == Path::new("-") {
                emit(&serde_json::to_value(&profile).map_err(|e| e.to_string())?)
            } else {
                profile.save(output)?;
                status(
                    cli,
                    json!({"ok":true,"output":output,"wardrobe":wardrobe_name}),
                    &format!("Saved wardrobe {}", output.display()),
                )
            }
        }
        WardrobeCommand::Apply {
            profile,
            source,
            output,
            format,
        } => {
            let mut source = source.clone();
            source.wardrobe.push(profile.clone());
            let recipe = source_recipe(&source, &cli.assets)?;
            cli_io::save(&recipe, output, format.id())?;
            if output != Path::new("-") {
                status(
                    cli,
                    json!({"ok":true,"output":output}),
                    &format!("Saved {}", output.display()),
                )?;
            }
            Ok(())
        }
        WardrobeCommand::Inspect { profile } => emit(
            &serde_json::to_value(WardrobeProfile::load(profile)?).map_err(|e| e.to_string())?,
        ),
        WardrobeCommand::Template { output, units } => {
            let lib = NativeLibrary::load(&cli.assets)?;
            write_bytes(
                output,
                crate::wardrobe_assets::body_template_obj(&lib, units)?.as_bytes(),
            )?;
            status(
                cli,
                json!({"ok":true,"output":output,"units":units,"coordinates":"MakeHuman Y-up, hip origin"}),
                &format!("Saved garment authoring template {}", output.display()),
            )
        }
        WardrobeCommand::Import {
            input,
            output,
            id,
            slot,
            units,
            surface_bind,
            color,
            texture,
            hide_faces,
        } => {
            if !crate::ara::valid_color(color) {
                return Err("garment color must be #RRGGBB".into());
            }
            let text = String::from_utf8(read_bounded(input)?).map_err(|_| "OBJ is not UTF-8")?;
            let lib = NativeLibrary::load(&cli.assets)?;
            let mut garment = crate::wardrobe_assets::import_obj_with_binding(
                &lib,
                &text,
                id,
                slot,
                units,
                *surface_bind,
            )?;
            garment.material["color"] = json!(color);
            if matches!(slot.as_str(), "hair" | "beard") {
                // Facial/scalp hair textures carry real alpha at strand edges
                // and atlas gutters. Opaque import turns those pixels black.
                garment.material["alpha"] = json!("mask");
                garment.material["doubleSided"] = json!(true);
            }
            if let Some(path) = hide_faces {
                garment.hidden_faces = serde_json::from_slice(&read_bounded(path)?)
                    .map_err(|e| format!("hidden face array: {e}"))?;
                let faces = lib.base.arrays["faceVerts"].u32()?.len() / 4;
                if garment.hidden_faces.iter().any(|&i| i as usize >= faces) {
                    return Err("garment hidden body face is out of bounds".into());
                }
            }
            let mut pending_texture = None;
            if let Some(path) = texture {
                let bytes = read_bounded(path)?;
                let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()
                    .map_err(|e| e.to_string())?;
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(8192);
                limits.max_image_height = Some(8192);
                limits.max_alloc = Some(256 * 1024 * 1024);
                reader.limits(limits);
                let image = reader
                    .decode()
                    .map_err(|e| format!("garment texture: {e}"))?;
                let mut png = std::io::Cursor::new(Vec::new());
                image
                    .write_to(&mut png, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                let relative = format!("textures/custom_{id}.png");
                let destination = cli.assets.join(&relative);
                if normalized(&destination)? == normalized(output)? {
                    return Err("garment definition and texture must have different paths".into());
                }
                pending_texture = Some((destination, png.into_inner()));
                garment.material["map"] = json!(relative);
            }
            garment.validate()?;
            let previous = if pending_texture.is_some() && output.exists() {
                Some(read_bounded(output)?)
            } else {
                None
            };
            write_bytes(
                output,
                &serde_json::to_vec_pretty(&garment).map_err(|e| e.to_string())?,
            )?;
            if let Some((destination, bytes)) = pending_texture {
                if let Err(error) = write_bytes(&destination, &bytes) {
                    let rollback = if let Some(previous) = previous {
                        write_bytes(output, &previous)
                    } else {
                        fs::remove_file(output).map_err(|e| e.to_string())
                    };
                    return Err(match rollback {
                        Ok(()) => error,
                        Err(rollback) => {
                            format!("{error}; restoring garment definition failed: {rollback}")
                        }
                    });
                }
            }
            status(
                cli,
                json!({"ok":true,"output":output,"id":id,"vertices":garment.positions_dm.len(),"triangles":garment.indices.len()/3}),
                &format!("Imported garment {id} into {}", output.display()),
            )
        }
        WardrobeCommand::Test {
            profile,
            input,
            preset,
            clip,
            samples,
            report,
            export_dir,
            contact_details,
            strict,
        } => {
            let wardrobe = WardrobeProfile::load(profile)?;
            if let Some(report) = report {
                if normalized(report)? == normalized(profile)? {
                    return Err("wardrobe report must not overwrite its profile".into());
                }
            }
            let mut bases = vec![];
            for path in input {
                bases.push((
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    cli_io::load(path, &cli.assets)?,
                ));
            }
            let presets = if input.is_empty() && preset.is_empty() {
                vec!["Athletic man".to_owned(), "Athletic woman".to_owned()]
            } else {
                preset.clone()
            };
            for name in presets {
                let source = Source {
                    preset: Some(name.clone()),
                    ..Default::default()
                };
                bases.push((name, source_recipe(&source, &cli.assets)?));
            }
            let lib = NativeLibrary::load(&cli.assets)?;
            let mut result = crate::wardrobe::test_profile_with_contact_details(
                &lib,
                &wardrobe,
                &bases,
                clip,
                *samples,
                contact_details.as_deref(),
            )?;
            if let Some(directory) = export_dir {
                let mut exports = vec![];
                for (index, (name, base)) in bases.iter().enumerate() {
                    if result["cases"][index]["ok"] != true {
                        continue;
                    }
                    let slug: String = name
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .take(64)
                        .collect();
                    let output = directory.join(format!("{:03}-{slug}.glb", index + 1));
                    if normalized(&output)? == normalized(profile)?
                        || report
                            .as_ref()
                            .is_some_and(|r| normalized(r).ok() == normalized(&output).ok())
                    {
                        return Err("wardrobe test artifacts must have distinct paths".into());
                    }
                    let mut recipe = base.clone();
                    wardrobe.apply(&mut recipe)?;
                    recipe.export.clips = clip.clone();
                    recipe.export.mode = "animated".into();
                    recipe.export.format = "glb".into();
                    let options = ExportArgs {
                        output,
                        format: Some("glb".into()),
                        mode: Some("animated".into()),
                        static_mesh: false,
                        clip: clip.clone(),
                        all_clips: false,
                        root_motion: Some(false),
                        save_recipe: None,
                    };
                    exports.push(perform_export(&lib, &recipe, &options)?);
                }
                result["exports"] = json!(exports);
            }
            if let Some(path) = report {
                write_bytes(
                    path,
                    &serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?,
                )?;
            }
            if !cli.quiet {
                emit(&result)?;
            }
            if result["ok"] != true
                || (*strict && result["warning_count"].as_u64().unwrap_or(0) > 0)
            {
                Err("wardrobe test failed; see the report for cases and fit warnings".into())
            } else {
                Ok(())
            }
        }
    }
}
