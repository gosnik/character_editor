"""Turn hair cards into one solid, smooth, chunky shell (run by build.py).

    blender --background --python tools/cartoon_hair/blender_shell.py -- JOB.json

JOB.json lists {"in": cards.npz, "out": shell.npz, "thickness", "voxel", "smooth",
"faces", "centre", "locks", "groove", "volume", "clear"}. The input holds P (n,3) and
F (m,3) triangles in hm08 decimetres, and H, the head surface points: the
hair cards plus a lifted copy of the scalp under them (it closes gaps between
sparse cards). The cards are thickened, merged by a voxel remesh, smoothed, given
volume and carved into locks (`locks`), then decimated to about `faces` triangles. The output holds P and F.
"""
import json
import sys

import bpy
import numpy as np



def locks(me, item):
    """Puff the shell out by `volume` and carve rounded locks: grooves running down
    from the crown, spaced around the head's vertical axis through `centre`, with a
    per-lock jitter, fading out on the crown where they would converge."""
    n = len(me.vertices)
    P = np.zeros(n * 3, np.float32); me.vertices.foreach_get("co", P); P = P.reshape(-1, 3)
    N = np.zeros(n * 3, np.float32); me.vertices.foreach_get("normal", N); N = N.reshape(-1, 3)
    d = P - np.array(item["centre"])
    d /= np.maximum(np.linalg.norm(d, axis=1, keepdims=True), 1e-9)
    k = item["locks"]
    t = (np.arctan2(d[:, 0], d[:, 2]) / (2 * np.pi) + 0.5) * k
    # Height above the head surface: grooves fade out where the shell is thin (the
    # hairline), and nothing is carved closer than `clear` to the skin, so the
    # partings never dip below it.
    from mathutils import kdtree
    H = item["_head"]
    tree = kdtree.KDTree(len(H))
    for i, h in enumerate(H):
        tree.insert(h, i)
    tree.balance()
    near = np.array([tree.find(p)[0] for p in P])
    up = P - near
    height = np.linalg.norm(up, axis=1)
    outer = (up * N).sum(1) > 0
    rng = np.random.default_rng(7)
    jitter = rng.uniform(-0.25, 0.25, k + 1)
    t = t + jitter[np.floor(t).astype(int) % (k + 1)] * (1 - np.abs(d[:, 1]))
    ridge = np.abs(np.sin(np.pi * t)) ** 0.6  # rounded locks, sharp partings between them
    fade = 1 - np.clip((d[:, 1] - 0.7) / 0.25, 0, 1)
    fade *= np.clip((height - item["clear"]) / (3 * item["clear"]), 0, 1)
    shift = item["volume"] + item["groove"] * (ridge - 0.75) * fade
    shift = np.where(outer, np.maximum(shift, item["clear"] - height), shift)
    P += N * shift[:, None]
    me.vertices.foreach_set("co", P.reshape(-1))
    me.update()


job = json.load(open(sys.argv[sys.argv.index("--") + 1]))
for item in job:
    d = np.load(item["in"])
    item["_head"] = d["H"].tolist()
    bpy.ops.wm.read_factory_settings(use_empty=True)
    me = bpy.data.meshes.new("cards")
    me.from_pydata(d["P"].tolist(), [], d["F"].tolist())
    ob = bpy.data.objects.new("cards", me)
    bpy.context.scene.collection.objects.link(ob)
    bpy.context.view_layer.objects.active = ob
    steps = [
        ("SOLIDIFY", dict(thickness=item["thickness"], offset=0.0, use_even_offset=False)),
        ("REMESH", dict(mode="VOXEL", voxel_size=item["voxel"], adaptivity=0.0)),
        ("SMOOTH", dict(factor=0.8, iterations=item["smooth"])),
        ("DECIMATE", dict(decimate_type="COLLAPSE", use_collapse_triangulate=True)),
    ]
    for kind, settings in steps:
        if kind == "DECIMATE":
            locks(ob.data, item)
        mod = ob.modifiers.new(kind.lower(), kind)
        for k, v in settings.items():
            setattr(mod, k, v)
        if kind == "DECIMATE":
            n = len(ob.data.polygons) * 2  # remesh output is quads
            mod.ratio = min(1.0, item["faces"] / max(n, 1))
        bpy.ops.object.modifier_apply(modifier=mod.name)
    me = ob.data
    n = len(me.vertices)
    P = np.zeros(n * 3, np.float32); me.vertices.foreach_get("co", P)
    sizes = np.zeros(len(me.polygons), np.int64); me.polygons.foreach_get("loop_total", sizes)
    loops = np.zeros(len(me.loops), np.int64); me.loops.foreach_get("vertex_index", loops)
    faces, k = [], 0
    for s in sizes:  # fan-triangulate anything the decimator left as a quad
        poly = loops[k:k + s]; k += s
        faces += [[poly[0], poly[i], poly[i + 1]] for i in range(1, s - 1)]
    np.savez(item["out"], P=P.reshape(-1, 3), F=np.array(faces, np.int64))
