# Headless command line

Human Studio's complete authoring CLI runs the native generator without creating
an application window, initializing a GPU, or requiring a display server. Node,
Python and Blender are not runtime dependencies. The CLI supports the full native
parameter catalog, detail morphs, wardrobe styles, breast shape/motion controls,
custom targets, MHM interchange, geometry inspection and batch export.

## Build and run

From `character_editor`:

```sh
cargo build --release --no-default-features --bin human-cli --bin human-export
./target/release/human-cli --help
```

`--no-default-features` excludes the editor, window event loop and X11 integration.
The native geometry/export engine still links Bevy's mesh/render libraries but
does not initialize a renderer. A graphics adapter and X11 development libraries
are unnecessary for the CLI-only feature configuration.

For development, use:

```sh
cargo run --no-default-features --bin human-cli -- catalog presets
```

A normal editor build also exposes exactly the same interface:

```sh
cargo run --bin human-editor -- --headless --help
cargo run --bin human-editor -- --headless export --preset 'Athletic woman' \
  --clip walk --output exports/woman.glb
```

`human-export` accepts its own flags (`--params`, `--out`, `--preset`, `--seed`,
`--clip`, `--all-clips`, `--static`, `--detail`, `--assets` and `--list`) as well as
the subcommands. `--list` prints the full catalog as JSON. Invalid/unknown arguments are rejected instead of ignored.

The default asset path points to this source checkout's `assets/native` directory.
When distributing binaries, ship that directory too, then use `--assets DIR` or
`HUMAN_EDITOR_ASSETS=DIR`. These are local assets; the CLI never downloads them.
For an embedded graphical game without the editor UI, enable the `window` feature
(`cargo run --no-default-features --features window --example embedded`).
The Basic editor commands use a separate `--library assets/makehuman` directory.

## Discover commands and controls

Every subcommand has `--help`. Global options may appear before or after a command.

```sh
human-cli catalog presets
human-cli catalog clips --filter walk
human-cli catalog fields --filter clothing
human-cli catalog sliders --filter breast
human-cli catalog garments --filter jacket
human-cli catalog outfit_sets
human-cli catalog assets
human-cli schema > schema.json
```

Catalog sections are `presets`, `clips`, `fields`, `sliders`, `garments`,
`outfit_sets`, `defaults`, `assets` and `all`. Entries include IDs, labels, ranges,
choices and pattern options. `schema` prints this catalog, native recipe defaults,
and additional breast control ranges; it is descriptive metadata, not a JSON
Schema validation document. Catalog and inspection output is always JSON.

## Create, edit and convert recipes

```sh
human-cli create --preset 'Athletic woman' --output woman.json
human-cli create --seed 42 --format project --output npc.project.json
human-cli edit --input woman.json --set height=0.7 --name 'Scout' --output scout.json
human-cli convert --input scout.json --format mhm --output scout.mhm
human-cli convert --input scout.mhm --format project --output scout.project.json
human-cli create --input - --output normalized.json < scout.json
```

`create`, `edit` and `convert` are aliases of the same operation. The optional
source is **one** of `--input`, `--preset`, or `--seed`. No source uses the native
default character. Inputs accept native recipe JSON, editor project JSON,
reference-style parameter JSON, or `.mhm` files. For projects in Basic mode,
the native commands restore `native_draft` if present; Basic-only projects use
`basic` commands instead. Native project conversion extracts the full native
recipe and creates a new project wrapper, rather than preserving Basic weights
or editor-only draft metadata.

Output formats are `native` (default), `project` and `mhm`. Recipe output defaults
to `-` (stdout), which allows piping; file outputs create parent directories and
use atomic replacement. `--format` controls recipe serialization explicitly,
independently of the filename. Standard MHM directives describe the interoperable
subset; the Human Studio comment extension preserves the complete native recipe.

New CLI recipes select the idle export clip by default. Loaded native/project/MHM
recipes retain their saved export settings unless overridden.

### Reusable wardrobes

Create a reusable wardrobe profile with `--wardrobe-name`. The flattened source
options retain `--name` for the character recipe, so the two names can be set
independently:

```sh
human-cli wardrobe create --preset 'Athletic woman' --name 'Scout' \
  --wardrobe-name 'Scout field kit' --output scout-field-kit.wardrobe.json
```

Apply it to another source without changing that source's body settings:

```sh
human-cli wardrobe apply scout-field-kit.wardrobe.json --preset 'Athletic man' \
  --output scout.json
```

### Cartoon characters

`cartoon.enabled` turns on Cartoon mode and `cartoon.strength` sets the amount
(0 realistic, 1 full cartoon). Wardrobes, rigs and clips work unchanged:

```sh
human-cli create --preset 'Athletic woman' --set cartoon.enabled=true \
  --set cartoon.strength=0.85 --wardrobe scout-field-kit.wardrobe.json \
  --output citizen.json
human-cli export --input citizen.json --output citizen.glb --clip idle,walk
```

See the README's *Cartoon mode* section for what it changes.

Meshy service animation IDs are `meshy-female:Idle`,
`meshy-female:Casual_Walk`, `meshy-female:Run_02` and their `meshy-male:`
counterparts. For example:

```sh
human-cli export --input citizen.json --output citizen.glb --clip meshy-female:Casual_Walk
```


### Author and test new garments

```sh
human-cli wardrobe template --output neutral-body.obj --units m
# Model a UV-mapped garment around that template, preserving its coordinates.
human-cli wardrobe import jacket.obj --id my_jacket --slot jacket \
  --output my-jacket.json --units m --texture jacket.png --color '#ffffff'
human-cli wardrobe create --preset 'Athletic woman' --garment my-jacket.json \
  --wardrobe-name 'Custom jacket' --output jacket.wardrobe.json
human-cli wardrobe test jacket.wardrobe.json --input citizen.json \
  --clip idle,walk --samples 5 --report fit-report.json --export-dir fit-models
```

The authoring template is Y-up at the original MakeHuman hip origin, rather than
the grounded game-export origin. OBJ import binds vertices to the shared body so
body changes and skinning carry through. OBJ materials are not imported; use the
color and texture options. Imported textures are copied into the selected native
asset library and must accompany it when distributing garments.

Use `--wardrobe PROFILE` and `--garment ASSET` with create, inspect, validate or
export. Parameter overrides are applied after wardrobe profiles. Batch jobs also
accept a `wardrobes` array with paths relative to the batch manifest. Use
`--remove-garment ID` to remove an imported layer.

Wardrobe tests check geometry and sample body/clothing intersections in selected
poses, including cartoon inputs. Reports include bounded hair-clearance warnings;
use `--strict` to return a failing exit status for warnings. These are fit diagnostics,
not continuous collision detection or cloth simulation. Exported test characters
allow visual inspection of flagged outfits.

Imported hair with `"fitHeadSurfaceClearanceMm": 3.0` can also set
`"fitHeadNeckClearance": true` when the source hairstyle extends onto the nape.
This includes the current neck surface (body region 1) alongside the scalp
(region 0) in the collision oracle. Hair bindings remain on the head; the
existing 35 mm movement limit and collider budgets still apply. Omitted or
explicitly false keeps the previous scalp-only behavior. The option requires
a Hair garment with valid `fitHeadSurfaceClearanceMm` and accepts only a boolean.
It works with both normal-gated clearance and the shared displacement field
selected by `fitHeadOuterCapGridDM`; visual and pose checks are still required.

Imported clothing can opt into bounded body clearance with material
`"fitClearanceMm": 3.0` (greater than zero and at most 10 mm). If clearance
needs more than the normal 40 mm source-relative correction budget,
an imported **Top**, **Bottom**, **Jacket**, or **Feet** garment may set
`"fitMaxCorrectionMm": 60.0` (greater than zero, at most 80 mm, and at least
the requested `fitClearanceMm`). The field requires a valid `fitClearanceMm`,
changes only that garment's body and layer clearance budget, and leaves the
omitted and explicit `40.0` cases on the
legacy 40 mm path. The fitter expands that garment's body collider reach to
the requested budget (60 mm uses a 0.7 DM collider margin). Nondefault requests
index only body triangles within the garment's possible movement and query
range; global geometry and candidate limits stay unchanged. Physical upper-waist tuck currently
rejects a nondefault correction limit rather than applying inconsistent
per-phase budgets. Thin-wall **Top** and **Jacket** garments using
`fitShellDisplacementPairs` may also use a nondefault limit. Their outer weld
aliases and recoupled lining targets share one source-snapshot total budget;
an incompatible pair is rejected instead of silently distorting the authored
wall relation.
For a reviewed exterior **Top** or **Jacket** whose ordinary clearance would
pull neighbouring vertices toward opposing body surfaces,
`"fitClearanceCoherentGridDM": 0.3` selects a shared trilinear correction
field. The grid must be 0.1–0.5 DM and requires valid `fitClearanceMm`.
It queries contacts from current fitted geometry but indexes field controls by
immutable authored positions, so exact source seam copies share one delta even
when body morphing separates their bindings. The existing source-relative
`fitMaxCorrectionMm` cap still applies (40 mm by default). It cannot be mixed
with triangle, excluded-vertex, upper-band/covered-tuck, shell, or layer
clearance metadata, because those modes add an independent correction.
For thin authored lining, `"fitClearanceExcludedVertices": [0, 1]` is an opt-in: it
excludes complete exact-position weld groups from body-clearance projection while
leaving all other vertices at the requested clearance. Indices must be unique,
in range, and include every UV sibling in each selected weld group.

Imported **Top** and **Jacket** garments may smooth animation skin weights with
`"fitSkinWeightSmoothingIterations": 1`. The field accepts an integer from 0
through 8; omitted or `0` preserves the legacy skinning result exactly. This is
a bounded, weights-only operation: positions, UVs, indices, and authored body
references remain unchanged. It averages only indexed mesh-edge neighbours and
exact-position weld aliases, so disconnected components do not influence each
other.

For a reviewed imported Top or Jacket with a small, known bad body-binding
island, use `"fitSkinWeightRepairVertices": [12, 13]`. It accepts at most 4096
unique in-range integer source vertex indices. Exact-position UV aliases are
welded and repaired together; only the marked island receives 64 harmonic
relaxation steps, while its unmarked mesh neighbours remain fixed boundary
weights. Every marked connected component must reach an unmarked boundary.
This preserves positions, UVs, indices, and authored body references. The
repair is mutually exclusive with `fitSkinWeightSmoothingIterations`; omitting
both fields preserves legacy skinning exactly.
smoothing inflates an already-clear waistband, add
`"fitClearanceSmoothingGuardMm": 3.0`. This fades neighbour smoothing over the
next 3 mm outside the requested clearance, using the current morphed body.
Direct collision correction remains active; source geometry, UVs and body
bindings are retained. The guard accepts values greater than zero and at most
10 mm and does not affect garments that omit it. To limit the guard to a
waistband, use `"fitClearanceSmoothingGuardUpperBand": [0.88, 0.96]`: activation
fades from zero to full strength between those fractions of the current morphed
garment height. The lower garment retains its ordinary smoothing. The band
requires a guard and two increasing fractions within 0 to 1.
To reduce the requested padding itself at a waistband, use
`"fitClearanceUpperBandMm": [0.88, 0.96, 2.0]`. This transitions from the base
clearance to 2 mm over the upper height band, retaining the base clearance below
it. The upper request must be positive and no greater than the base request.
It applies to vertex and optional triangle contact corrections.
For custom pants that need this reduced band only beneath an opaque higher
custom layer, add `"fitClearanceUpperBandCoveredOnly": true`. The garment then
uses its base `fitClearanceMm` at every upper-band vertex unless every meaningful
canonical body anchor is covered by a higher-ranked custom top or jacket with
`"fitOccludesGarments": true`. Coverage combines that outer garment's saved
body-face mask with its final runtime candidate mask after ordinary clearance;
a cropped top therefore leaves an exposed waistband at the base clearance.
The field requires `fitClearanceUpperBandMm`, rejects shell displacement pairs
and `fitLayerClearanceMm` on the same garment,
and preserves the existing upper-band behavior when omitted or set to false.
For an imported **Bottom** whose covered waistband remains visibly proud of the
body, the same strict opt-in may additionally use
`"fitClearanceUpperBandCoveredTuckMm": 20.0` (greater than zero and at most
40 mm). This pulls only fully covered source-weld groups inward from their
post-clearance position, over the same source-Y upper-band activation, toward
the nearest visible-body surface. It never crosses that band's requested
clearance floor, moves an uncovered UV-weld sibling, changes vertices below
the band, or exceeds the existing 4 cm source-relative correction budget. The
field requires both `fitClearanceUpperBandCoveredOnly: true` and a valid base
and upper-band clearance request; malformed dependencies fail before the part
is changed. It is intended for an opaque outer top or jacket over a Bottom,
not as a general garment shrinker. The fitter runs a bounded contact repair on
triangles touched by the tuck, but sampled runtime collision checks remain
required for an imported source mesh.
For a **Bottom** whose authored wall is physically under an opaque higher mesh
but whose canonical anchors straddle that mesh's hem, additionally set
`"fitClearanceUpperBandPhysicalCoverage": true`. This requires the covered-only
flag and a positive covered-tuck request. After canonical fitting settles, it
checks every whole source-weld group against the full morphed body and final
higher opted-in opaque custom meshes. A group moves inward only when every
sibling is in the upper source-Y band and has a forward outer-mesh hit within
8 cm with room for the floor on both surfaces. A source wall that is already
slightly inside the full-body floor can instead move outward only far enough to
restore that floor after the same proof; this is not a general body-clearance
pass. The motion follows the body normal, retains tangential source offset,
uses the same 40 mm request and shared 4 cm source-relative budget, and restores
a group to its canonical-fit position if bounded contact repair or a final
body/outer triangle check cannot retain both surfaces' room. Omit the boolean
for unchanged existing covered-tuck behavior.
Check the resulting silhouette
and sampled animations: bounded clearance does not guarantee an intersection-free
outfit or resolve collisions between clothing layers.

Imported custom bottoms, tops, and jackets can opt into a small bind-pose
separation from lower ranked custom clothing with
`"fitLayerClearanceMm": 3.0` (greater than zero and at most 10 mm). The fitter
uses the current full body only for an outward direction, then tests lower
clothing along that direction and moves welded outer vertices only as far as
the shared 4 cm source-relative correction budget allows. It runs after body
and shell clearance, before animation bindings and coverage masking. It does
not include hidden body anatomy as a clothing collider, alter UVs, topology,
bindings, or saved asset geometry, and it is not cloth simulation. Garments
with shell displacement pairs reject this opt-in because their lining must
remain coupled to their outer controls. Omit the field to preserve existing
fitting exactly.

An opaque imported top, jacket or bottom can request layer coverage masking with
`"fitOccludesGarments": true`. Fully covered triangles of lower custom clothing
are omitted from the assembled preview and export. Saved garment geometry, UVs
and bindings are retained. If independently fitted layers cross slightly, the
outer garment can additionally set `"fitOccludesGarmentsInwardMm": 20.0` (0 to
40 mm) to give it priority within that overlap. Coverage still samples each lower
triangle, retaining exposed hems and openings. Omission keeps the existing
forward-only coverage rule. This is a bounded rendering/export coverage heuristic;
it does not move cloth or simulate collisions during animation. Check the actual
outfit from several angles and in motion before accepting the asset.

### Customize every control

```sh
human-cli create --preset 'Athletic woman' \
  --set gender=0 --set muscle=0.65 --set weight=0.45 \
  --set breastSize=0.8 --set breasts.volume=1.8 --set breasts.projection=1.5 \
  --set breasts.lift=0.3 --set breasts.separation=0.15 \
  --set breasts.dynamics=true --set breasts.stiffness=30 \
  --set breasts.damping=0.8 --set breasts.motion_strength=0.7 \
  --rig game --rest-pose T --detail medium --output character.json
```

`--set NAME=VALUE` is repeatable. Values are parsed as JSON where possible;
otherwise they are strings. Quote JSON strings explicitly when a numeric-looking
string is intended (`--set 'name="123"'`). Arrays and objects are JSON:

```sh
human-cli edit --input character.json \
  --set outfit=none --set top=tank_top --set bottom=cargo_shorts \
  --set 'accessories=["watch"]' \
  --style tank_top.color='#223344' --style tank_top.color2='#d0b060' \
  --style tank_top.pattern=solid --output dressed.json
```

Use discovered IDs/allowed patterns from `catalog`; not every garment supports
every pattern. Additional property spellings:

- `height=0.7` or `params.height=0.7` for parameters.
- `breasts.volume=1.8` for the additional breast settings.
- `export.mode=rigged`, `export.root_motion=true`,
  `export.clips=["walk","run"]` for persisted export defaults.
- `/preview_clip=walk` or `/params/height=0.7` for JSON pointers.
- `--modifier ID=WEIGHT` for the 219 detail sliders. Discover the exact ID with
  `catalog sliders`; equivalent pointer paths are `/params/modifiers/ID`.
  Escape `/` in a pointer component as `~1` and `~` as `~0`.
- `--style GARMENT.FIELD=VALUE` for `color`, `color2`, `pattern`;
  equivalent spelling is `--set garmentStyles.GARMENT.FIELD=VALUE`.

Unknown fields, slider IDs, invalid choices/ranges, colors and patterns fail with
an error. Setters are applied in command-line order within each option group:
`--set`, detail modifiers, styles, convenience name/detail/rig/rest-pose options,
then custom-target operations. Export flags take precedence over recipe settings.
To make several related changes atomically, provide a complete JSON object in
one assignment or edit a recipe file and validate it.

### Custom targets

```sh
human-cli edit --input character.json --target ./shape.target.gz \
  --target-weight shape.target.gz=0.65 --mirror-target shape.target.gz \
  --output sculpted.json
human-cli edit --input sculpted.json --remove-target 'shape.target.gz mirrored' \
  --output sculpted.json
human-cli target inspect ./shape.target.gz --json
human-cli target mirror ./shape.target.gz --output ./shape_mirrored.target.gz
```

Target names default to their filenames. Names must be distinct when imported;
mirror operations append ` mirrored`. Weights must be in `[-1,1]`. Removal happens
before imports, weight updates and mirroring. Mirroring uses the bundled original
hm08 coordinates and X symmetry; `--tolerance` is in MakeHuman decimeters.
Targets use original hm08 vertex indices and Y-up XYZ decimeters. Gzip inputs and
mirrored gzip outputs are supported. A native recipe/project stores imported
deltas so no external target file is needed when reopened.

## Validate and inspect

```sh
human-cli validate --input character.json --json
human-cli validate --input character.json --build --json
human-cli inspect --input character.json --recipe-only > normalized.json
human-cli inspect --input character.json > geometry.json
```

Default validation checks the recipe without constructing geometry. `--build`
also resolves assets and constructs the character. Inspection includes bounds in
meters, height, triangle/vertex counts, part materials, bone hierarchy and clip
durations. `--recipe-only` avoids geometry loading. Camera controls and editor
undo history are UI state and do not apply to headless authoring.

## Export

```sh
human-cli export --input character.json --output exports/character.glb \
  --mode animated --clip idle,walk,run --root-motion \
  --save-recipe exports/character.recipe.json --json
human-cli export --preset 'Athletic woman' --output exports/rigged.glb --mode rigged
human-cli export --seed 42 --output exports/static.glb --mode static
human-cli export --input character.json --output exports/character.obj
human-cli export --input character.json --output exports/all-motion.glb --all-clips
```

GLB embeds textures, materials, optional skin/face shapes and chosen animation
clips. Modes are `static`, `rigged`, `animated`. OBJ exports the static current
shape plus MTL/textures; it does not store rigs or clips. The format is inferred
from `.glb`/`.obj`, or supplied with `--format`; a conflicting extension fails.
`--static` aliases `--mode static`. Repeated `--clip` or comma-separated IDs
replace the saved selection; `--all-clips` chooses all 77 and conflicts with
`--clip`. Root translation accepts `--root-motion`, `--root-motion=true` or
`--root-motion=false`. Disabling root motion removes horizontal travel while
retaining vertical pelvis motion and jumps. Independent breast secondary motion is baked into animated
GLBs when enabled. `--save-recipe` writes the exact native recipe used for export.

Mesh outputs and saved export recipes must be files, not stdout, and must have
distinct paths. Output files may replace existing files. Ordinary recipe files
and GLB writes are atomic; OBJ's MTL/texture sidecars are separate file writes.

## Batch exports

```json
{
  "version": 1,
  "jobs": [
    {
      "preset": "Athletic woman",
      "set": ["height=0.6", "breasts.volume=1.4"],
      "output": "exports/woman.glb",
      "clips": ["idle", "walk"],
      "mode": "animated",
      "root_motion": false,
      "save_recipe": "exports/woman.json"
    },
    {
      "seed": 42,
      "set": ["rig=game_lite"],
      "output": "exports/npc.obj"
    }
  ]
}
```

```sh
human-cli batch --manifest batch.json --report results.json --json
human-cli batch --manifest batch.json --continue-on-error --json
```

Each job chooses at most one of `input`, `preset`, `seed`, and may provide `set`,
`clips`, `mode`, `root_motion`, `save_recipe`. Unknown manifest/job fields fail.
A manifest requires version 1 and 1–10,000 jobs. Inputs and output artifacts are
relative to the manifest directory; `--report` and `--assets` are relative to the
shell's working directory. A stdin manifest (`--manifest -`) uses the working
directory; job inputs must be files. Duplicate mesh/recipe/report output paths
are rejected before generation. Jobs share one asset library and process caches.

Default behavior stops after the first failed job; `--continue-on-error` attempts
the rest. Results identify each processed job by its zero-based index. Any failed
job gives exit 1 even with `--continue-on-error`; successful files remain on disk.
The report is written for job success/failure results, including fail-fast runs;
invalid manifests or preflight failures report an error before running jobs.
Batch execution does not provide a multi-file transaction.

## Original Basic editor projects

```sh
human-cli basic catalog
human-cli basic create --name 'Basic NPC' --weight height=0.4 \
  --skin '#8b6f57' --output basic.json
human-cli basic validate --input basic.json --json
human-cli basic inspect --input basic.json
human-cli basic export --input basic.json --weight height=0.7 --output basic.obj
```

These commands use the original lightweight library and its target IDs, which
are separate from the native detail slider IDs. Use `basic catalog` to discover
these controls. `basic create` may read an existing Basic project to edit it;
without `--output` it writes JSON to stdout. Other Basic actions require `--input`.
Projects with active native customization are rejected here. An inactive
`native_draft` is retained when editing Basic projects; Basic exports use the
active Basic shape and report draft presence. Native commands restore that draft
when full customization is desired.

## Script behavior

- Exit **0**: success, help, version.
- Exit **1**: invalid recipe, missing asset/file, failed build/export, failed batch job.
- Exit **2**: invalid command-line syntax or conflicting flags.

Use `--json` for JSON status records on stdout and JSON runtime errors on stderr.
Catalog, schema, inspection and stdout recipes are already JSON (MHM recipes are
text). Clap syntax/help diagnostics use ordinary text on the appropriate stream.
The batch command prints one result summary on stdout, then reports a runtime
error on stderr if any job failed. `--quiet` suppresses status summaries but
preserves requested recipe/catalog/inspection output and errors. Source JSON and
manifests are limited to 64 MiB; target decompression has its own bounded parser.

## Universal Animation Libraries

Download the Quaternius Universal Animation Library packs (CC0 1.0; not included in
this repository) and import them once into the selected native asset directory:

```sh
human-cli animation-import --id ual1 --source '../Universal Animation Library'
human-cli animation-import --id ual2 --source '../Universal Animation Library 2'
human-cli catalog clips --filter ual1
human-cli create --set preview_clip=ual1:Walk_Loop --output walker.json
human-cli export --input walker.json --clip ual1:Walk_Loop --root-motion true --output walker.glb
```

The importer reads the Unreal-Godot GLB and its `_RM` companion and retains
namespaced clip IDs, so duplicate animation names across libraries do not collide.
The imported catalog is also available in the GUI animation selector. Restart an
open editor after importing a library. Recipes keep their existing export selection;
use `--all-clips` or the GUI's export-all button to select every installed clip.
Keep the packs' CC0 license files alongside the source assets.

### Sleeve cage garment metadata

Top and jacket custom garments may opt into `fitSleeveCage` when an authored puff
sleeve needs a shared two-segment arm deformation instead of independent body
triangle offsets. The object has one or two `arms`; each arm declares its
non-overlapping `vertexIndices`, canonical `shoulderRefs`, `elbowRefs`, and
`wristRefs` rings (3–64 unique arm-region vertices each), plus `rootBlendDM`
and `elbowBlendDM` in `0.05..=1.0`. Optional `vertexWeights` must match
`vertexIndices`; each finite `0..=1` weight feathers the cage result, and zero
keeps that vertex on the exact legacy mapping. The cage derives neutral/current centres,
rotation, axial scale, and radial scale from those rings, maps selected source
coordinates through shoulder–elbow and elbow–wrist segments, and smooths only
across the authored root and elbow bands. It never projects vertices onto the
body surface. All aliases at the same authored source position must be assigned
to the same arm with equal weights, or all left unselected. The neutral ring
centres must progress outward from shoulder to wrist on the same body side.
It is rejected for slots other than top/jacket and cannot be combined
with `fitTorsoCage` or `fitCoherentBodyAnchor`; absent metadata keeps legacy
adaptive fitting exactly.

An arm may set `radialMode: "preserve-ease"` for a loose sleeve. The normal
default maps every radial offset by the body-radius ratio. `preserve-ease` keeps
that ratio through the neutral body radius, then adds the current-minus-neutral
body radius to larger offsets. It therefore lets a close cuff follow arm girth
while retaining an authored puff's clearance in DM instead of scaling the whole
puff volume. Omitting `radialMode` retains the original body-ratio mapping.
