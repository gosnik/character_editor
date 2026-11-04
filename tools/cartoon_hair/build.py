"""Build the cartoon hair shells: one solid, chunky proxy per MakeHuman hair.

    python3 tools/cartoon_hair/build.py [HAIR_ID ...]

For each hair in assets/native/catalog.json the cards are evaluated on the hm08 base
mesh, joined with a slightly lifted copy of the scalp under them (so sparse cards
leave no holes), then thickened, voxel-remeshed, smoothed and decimated in Blender
(`blender_shell.py`; `BLENDER` or `blender` on PATH). Each shell vertex borrows the
proxy binding (three body vertices and weights) of the nearest original card vertex,
with its own offset, so the shell follows every body morph (cartoon head included) the
way the cards did. The colour is the cards' mean texture colour, as a flat matte
material; the editor's hair tint multiplies it as before.

Writes assets/native/proxies/cartoon_hair_<id>.pack, which the editor uses in place of
the cards in cartoon mode (see `native_character`).
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.spatial import cKDTree

HERE = Path(__file__).resolve().parent
CE = HERE.parent.parent
NATIVE = CE / "assets/native"
sys.path.insert(0, str(HERE.parent / "cartoon_hm08"))
import mhp1  # noqa: E402

BODY_VERTS = 13380  # hm08 body vertices; helper geometry follows
SCALP_REACH = 0.12  # dm: scalp vertices this close to a card join the shell
SCALP_LIFT = 0.02  # dm
THICKNESS = 0.07  # dm card thickening before the remesh
VOXEL = 0.035  # dm
SMOOTH = 30
FACES = 3200
LOCKS = 18  # around the head
GROOVE = 0.13  # dm, ridge-to-parting depth
CLEAR = 0.03  # dm: the shell's outside stays this far above the skin
VOLUME = 0.05  # dm puff along the normal (also undoes smoothing shrink)


def tris(sizes, verts):
    out, k = [], 0
    for s in sizes.astype(int):
        f = verts[k:k + s]; k += s
        out += [[f[0], f[i], f[i + 1]] for i in range(1, s - 1)]
    return np.array(out, np.int64)


def vertex_normals(P, F):
    n = np.zeros_like(P)
    fn = np.cross(P[F[:, 1]] - P[F[:, 0]], P[F[:, 2]] - P[F[:, 0]])
    for k in range(3):
        np.add.at(n, F[:, k], fn)
    return n / np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)


def mean_colour(material):
    tex = material.get("diffuse") or material.get("map")
    im = np.asarray(Image.open(NATIVE / "textures" / tex).convert("RGBA"), np.float32)
    # Strand textures are mostly dark shading between strands: take the lit half.
    a = im[..., 3] > 128
    rgb = im[..., :3][a] if a.any() else np.full((1, 3), 128.0)
    lum = rgb @ [0.299, 0.587, 0.114]
    c = rgb[lum >= np.median(lum)].mean(0)
    tint = material.get("color", "#ffffff").lstrip("#")
    c = c * np.array([int(tint[i:i + 2], 16) for i in (0, 2, 4)]) / 255
    return "#" + "".join(f"{int(round(x)):02x}" for x in c)


def main(only):
    catalog = json.loads((NATIVE / "catalog.json").read_text())
    meta_b, base = mhp1.read(NATIVE / "base.pack")
    B = base["positions"].reshape(-1, 3)
    body_quads = base["faceVerts"].reshape(-1, 4)
    body_quads = body_quads[(body_quads < BODY_VERTS).all(1)]
    body_tris = np.concatenate([body_quads[:, [0, 1, 2]], body_quads[:, [0, 2, 3]]])
    normals = vertex_normals(B, body_tris)
    neck_y = B[meta_b["joints"]["head____head"]].mean(0)[1]
    # Locks radiate from a point inside the skull, a little above the ears.
    head = B[:BODY_VERTS][B[:BODY_VERTS, 1] > neck_y]
    centre = [float(head[:, 0].mean()), float(np.percentile(head[:, 1], 60)), float(head[:, 2].mean())]
    work = Path(tempfile.mkdtemp(prefix="cartoon_hair_"))
    jobs, items = [], []
    for item in catalog["hair"]:
        if only and item["id"] not in only:
            continue
        meta, a = mhp1.read(NATIVE / item["file"])
        sc = np.array([abs(B[va, d] - B[vb, d]) / den for d, (va, vb, den) in enumerate(meta["scale"])])
        refs, w = a["refs"].reshape(-1, 3), a["weights"].reshape(-1, 3)
        cards = (B[refs] * w[..., None]).sum(1) + a["offsets"].reshape(-1, 3) * sc
        F = tris(a["faceSizes"], a["faceVerts"])
        # Scalp under the hair (head only: long hair also lies on the shoulders, which move apart).
        near, _ = cKDTree(cards).query(B[:BODY_VERTS], distance_upper_bound=SCALP_REACH)
        under = np.isfinite(near) & (B[:BODY_VERTS, 1] > neck_y)
        patch = body_quads[under[body_quads].all(1)]
        ids = np.unique(patch)
        remap = np.full(len(B), -1); remap[ids] = np.arange(len(ids)) + len(cards)
        P = np.concatenate([cards, B[ids] + normals[ids] * SCALP_LIFT])
        F = np.concatenate([F, remap[patch[:, [0, 1, 2]]], remap[patch[:, [0, 2, 3]]]])
        np.savez(work / f"{item['id']}.in.npz", P=P, F=F, H=head)
        jobs.append({"in": str(work / f"{item['id']}.in.npz"), "out": str(work / f"{item['id']}.out.npz"),
                     "thickness": THICKNESS, "voxel": VOXEL, "smooth": SMOOTH, "faces": FACES,
                     "centre": centre, "locks": LOCKS, "groove": GROOVE, "volume": VOLUME, "clear": CLEAR})
        items.append((item, meta, refs, w, cards, sc))
    (work / "job.json").write_text(json.dumps(jobs))
    subprocess.run([os.environ.get("BLENDER", "blender"), "--background", "--python",
                    str(HERE / "blender_shell.py"), "--", str(work / "job.json")],
                   check=True, stdout=subprocess.DEVNULL)
    for (item, meta, refs, w, cards, sc), job in zip(items, jobs):
        d = np.load(job["out"]); P, F = d["P"].astype(np.float64), d["F"]
        _, nearest = cKDTree(cards).query(P)
        r, wt = refs[nearest], w[nearest]
        offsets = (P - (B[r] * wt[..., None]).sum(1)) / sc
        material = {"name": f"cartoon_{item['id']}", "color": mean_colour(item["material"]),
                    "roughness": 0.6, "metalness": 0, "alpha": "opaque", "doubleSided": False}
        out_meta = {"id": f"cartoon_{item['id']}", "kind": "hair", "zDepth": meta.get("zDepth", 50),
                    "scale": meta["scale"], "material": material, "vertexCount": int(len(P))}
        mhp1.write(NATIVE / "proxies" / f"cartoon_hair_{item['id']}.pack", out_meta, {
            "refs": r.reshape(-1).astype(np.uint32), "weights": wt.reshape(-1).astype(np.float32),
            "offsets": offsets.reshape(-1).astype(np.float32),
            "faceSizes": np.full(len(F), 3, np.uint8), "faceVerts": F.reshape(-1).astype(np.uint32),
            "faceUVs": np.zeros(F.size, np.uint32), "uvs": np.array([0.5, 0.5], np.float32),
            "deleteVerts": np.zeros(0, np.uint32),
        })
        print(f"{item['id']}: {len(cards)} card verts -> shell {len(P)} verts, {len(F)} tris, {material['color']}")


if __name__ == "__main__":
    main(set(sys.argv[1:]))
