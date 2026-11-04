//! MakeHuman OBJ topology and sparse target deformation, independent of rendering.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("Invalid character data: {0}")]
    Invalid(String),
}
type Result<T> = std::result::Result<T, ModelError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CharacterRecipe {
    pub version: u32,
    pub name: String,
    pub weights: BTreeMap<String, f32>,
    pub skin_color: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customization: Option<crate::ara::AraRecipe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_draft: Option<crate::ara::AraRecipe>,
}
impl Default for CharacterRecipe {
    fn default() -> Self {
        Self {
            version: 1,
            name: "New character".into(),
            weights: BTreeMap::new(),
            skin_color: [0.57, 0.36, 0.25],
            customization: None,
            native_draft: None,
        }
    }
}
impl CharacterRecipe {
    pub fn enter_basic_editor(&mut self) {
        if self.customization.is_some() {
            self.native_draft = self.customization.take();
        }
    }
    pub fn enter_full_editor(&mut self) {
        if self.customization.is_none() {
            self.customization = Some(self.native_draft.take().unwrap_or_default());
        }
    }

    pub fn load(path: impl AsRef<Path>, library: &CharacterLibrary) -> Result<Self> {
        let recipe: Self = serde_json::from_str(&fs::read_to_string(path)?)?;
        library.validate(&recipe)?;
        Ok(recipe)
    }
    pub fn save(&self, path: impl AsRef<Path>, library: &CharacterLibrary) -> Result<()> {
        library.validate(self)?;
        atomic_write(
            path.as_ref(),
            serde_json::to_string_pretty(self)?.as_bytes(),
        )
    }
}

/// Replace only after a complete write; a failed save leaves the previous file intact.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| ModelError::Invalid("missing output filename".into()))?
        .to_string_lossy();
    // create_new also protects against existing files/symlinks at temporary paths.
    let (temp, mut file) = loop {
        let nonce = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let result = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result.map_err(Into::into)
}

fn read_asset(root: &Path, relative: &str) -> Result<String> {
    if Path::new(relative)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(ModelError::Invalid(
            "asset paths must be relative without traversal".into(),
        ));
    }
    let root = root.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) {
        return Err(ModelError::Invalid("asset path escapes library".into()));
    }
    Ok(fs::read_to_string(path)?)
}

#[derive(Clone, Debug, Deserialize)]
pub struct TargetDefinition {
    pub id: String,
    pub label: String,
    pub category: String,
    pub negative: String,
    pub positive: String,
}
#[derive(Deserialize)]
struct Manifest {
    source_commit: String,
    targets: Vec<TargetDefinition>,
}
#[derive(Clone, Debug)]
pub struct MorphTarget {
    pub definition: TargetDefinition,
    negative: Vec<(usize, [f32; 3])>,
    positive: Vec<(usize, [f32; 3])>,
}
#[derive(bevy::prelude::Resource, Clone, Debug)]
pub struct CharacterLibrary {
    pub source_commit: String,
    base_positions: Vec<[f32; 3]>,
    triangles: Vec<u32>,
    pub targets: Vec<MorphTarget>,
}
impl CharacterLibrary {
    pub fn base_positions(&self) -> &[[f32; 3]] {
        &self.base_positions
    }
    pub fn triangles(&self) -> &[u32] {
        &self.triangles
    }
    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let root = directory.as_ref();
        let manifest: Manifest = serde_json::from_str(&read_asset(root, "manifest.json")?)?;
        let (base_positions, triangles) = parse_obj(&read_asset(root, "data/3dobjs/base.obj")?)?;
        let mut targets = Vec::new();
        for definition in manifest.targets {
            if targets
                .iter()
                .any(|t: &MorphTarget| t.definition.id == definition.id)
            {
                return Err(ModelError::Invalid("duplicate target ID".into()));
            }
            let negative = parse_target(
                &read_asset(root, &definition.negative)?,
                base_positions.len(),
            )?;
            let positive = parse_target(
                &read_asset(root, &definition.positive)?,
                base_positions.len(),
            )?;
            targets.push(MorphTarget {
                definition,
                negative,
                positive,
            });
        }
        Ok(Self {
            source_commit: manifest.source_commit,
            base_positions,
            triangles,
            targets,
        })
    }
    pub fn validate(&self, recipe: &CharacterRecipe) -> Result<()> {
        if let Some(draft) = &recipe.native_draft {
            draft.validate().map_err(ModelError::Invalid)?;
        }
        if let Some(customization) = &recipe.customization {
            customization.validate().map_err(ModelError::Invalid)?;
        }
        if recipe.version != 1 {
            return Err(ModelError::Invalid("unsupported recipe version".into()));
        }
        if recipe.name.len() > 256 {
            return Err(ModelError::Invalid("name exceeds 256 bytes".into()));
        }
        for (id, weight) in &recipe.weights {
            if !weight.is_finite()
                || !(-1.0..=1.0).contains(weight)
                || !self.targets.iter().any(|t| t.definition.id == *id)
            {
                return Err(ModelError::Invalid(format!(
                    "unknown target or invalid weight: {id}"
                )));
            }
        }
        if recipe
            .skin_color
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(ModelError::Invalid("invalid skin color".into()));
        }
        Ok(())
    }
    /// Targets are MakeHuman Y-up decimeter offsets, converted to Bevy meters.
    /// Always evaluate from the immutable base to avoid cumulative slider drift.
    pub fn deform(&self, recipe: &CharacterRecipe) -> Result<Vec<[f32; 3]>> {
        self.validate(recipe)?;
        let mut positions = self.base_positions.clone();
        for target in &self.targets {
            let weight = recipe
                .weights
                .get(&target.definition.id)
                .copied()
                .unwrap_or(0.0);
            let offsets = if weight < 0.0 {
                &target.negative
            } else {
                &target.positive
            };
            for &(index, delta) in offsets {
                for axis in 0..3 {
                    positions[index][axis] += delta[axis] * weight.abs() * 0.1;
                }
            }
        }
        // Re-ground after each edit, using visible body geometry only.
        let floor = self
            .triangles
            .iter()
            .map(|&i| positions[i as usize][1])
            .fold(f32::INFINITY, f32::min);
        for p in &mut positions {
            p[1] -= floor;
        }
        Ok(positions)
    }
    pub fn export_obj(&self, recipe: &CharacterRecipe, path: impl AsRef<Path>) -> Result<()> {
        use std::fmt::Write;
        let positions = self.deform(recipe)?;
        let mut output =
            String::from("# MakeHuman-derived character; meters, Y-up; visible geometry only\n");
        // Compact helper vertices out of exported geometry.
        let mut mapping = BTreeMap::new();
        for &i in &self.triangles {
            if !mapping.contains_key(&i) {
                let next = mapping.len() + 1;
                mapping.insert(i, next);
                let p = positions[i as usize];
                writeln!(output, "v {} {} {}", p[0], p[1], p[2]).unwrap();
            }
        }
        for t in self.triangles.chunks_exact(3) {
            writeln!(
                output,
                "f {} {} {}",
                mapping[&t[0]], mapping[&t[1]], mapping[&t[2]]
            )
            .unwrap();
        }
        atomic_write(path.as_ref(), output.as_bytes())
    }
}

pub fn parse_target(text: &str, vertex_count: usize) -> Result<Vec<(usize, [f32; 3])>> {
    let mut deltas = BTreeMap::new();
    for (line, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with(['#', '"']) {
            continue;
        }
        let fields: Vec<_> = raw.split_whitespace().collect();
        let invalid = || ModelError::Invalid(format!("target line {}", line + 1));
        if fields.len() != 4 {
            return Err(invalid());
        }
        let index: usize = fields[0].parse().map_err(|_| invalid())?;
        let mut delta = [0.0; 3];
        for axis in 0..3 {
            delta[axis] = fields[axis + 1].parse::<f32>().map_err(|_| invalid())?;
            if !delta[axis].is_finite() {
                return Err(invalid());
            }
        }
        if index >= vertex_count || deltas.insert(index, delta).is_some() {
            return Err(invalid());
        }
    }
    Ok(deltas.into_iter().collect())
}

pub fn parse_obj(text: &str) -> Result<(Vec<[f32; 3]>, Vec<u32>)> {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let mut visible = true;
    for raw in text.lines() {
        let fields: Vec<_> = raw.split_whitespace().collect();
        match fields.first().copied() {
            Some("v") => {
                if fields.len() < 4 {
                    return Err(ModelError::Invalid("OBJ vertex".into()));
                }
                let mut p = [0.0; 3];
                for axis in 0..3 {
                    p[axis] = fields[axis + 1]
                        .parse::<f32>()
                        .map_err(|_| ModelError::Invalid("OBJ number".into()))?
                        * 0.1;
                    if !p[axis].is_finite() {
                        return Err(ModelError::Invalid("nonfinite OBJ vertex".into()));
                    }
                }
                positions.push(p);
            }
            Some("g") => {
                visible = !fields[1..]
                    .iter()
                    .any(|g| g.starts_with("joint") || g.starts_with("helper"));
            }
            Some("f") if visible => {
                if fields.len() < 4 {
                    return Err(ModelError::Invalid("OBJ face".into()));
                }
                let mut face = Vec::new();
                for f in &fields[1..] {
                    let index: usize = f
                        .split('/')
                        .next()
                        .unwrap()
                        .parse()
                        .map_err(|_| ModelError::Invalid("OBJ index".into()))?;
                    if index == 0 || index > positions.len() {
                        return Err(ModelError::Invalid("OBJ index out of bounds".into()));
                    }
                    face.push((index - 1) as u32);
                }
                for n in 1..face.len() - 1 {
                    triangles.extend([face[0], face[n], face[n + 1]]);
                }
            }
            _ => {}
        }
    }
    if triangles.is_empty() {
        return Err(ModelError::Invalid("OBJ contains no visible faces".into()));
    }
    Ok((positions, triangles))
}
