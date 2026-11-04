"""Carry MakeHuman .target deltas onto the cartoon hm08 bases.

Each vertex gets a deformation gradient J (3x3) from the realistic hm08 shape to the
fitted cartoon shape, estimated by least squares over its 2-ring edge vectors plus its
normal, then smoothed.  A realistic delta d becomes J d, so a nose target scales with the
cartoon nose, a head target with the enlarged head, and so on.
"""
import gzip
from pathlib import Path
import numpy as np
from geom import tri_list, vertex_normals


def adjacency(o, n):
    adj = [set() for _ in range(n)]
    for f in o.faces:
        for i in range(len(f)):
            a, b = f[i], f[(i + 1) % len(f)]
            adj[a].add(b); adj[b].add(a)
    return adj


def deformation_gradients(o, A, B, smooth=3):
    n = len(A); adj = adjacency(o, n)
    T = tri_list(o.faces); NA = vertex_normals(A, T); NB = vertex_normals(B, T)
    J = np.tile(np.eye(3), (n, 1, 1))
    for i in range(n):
        ring = set(adj[i])
        for r in list(ring):
            ring |= adj[r]
        ring.discard(i); ring = list(ring)
        if not ring:
            continue
        ea = A[ring] - A[i]; eb = B[ring] - B[i]
        la = np.linalg.norm(ea, axis=1).mean(); lb = np.linalg.norm(eb, axis=1).mean()
        ea = np.vstack([ea, NA[i] * la]); eb = np.vstack([eb, NB[i] * lb])
        # J ea^T = eb^T  (least squares, lightly regularised toward isotropic scale)
        s = lb / max(la, 1e-9); lam = 1e-3 * la * la
        M = ea.T @ ea + lam * np.eye(3); R = eb.T @ ea + lam * s * np.eye(3)
        J[i] = R @ np.linalg.inv(M)
    for _ in range(smooth):
        J = np.stack([(J[i] + J[list(adj[i])].sum(0)) / (1 + len(adj[i])) if adj[i] else J[i] for i in range(n)])
    return J


def gaussian_operator(V, sigma):
    """Row-normalised spatial Gaussian smoothing matrix over the realistic shape."""
    import scipy.sparse as sp
    from scipy.spatial import cKDTree
    t = cKDTree(V); D = t.sparse_distance_matrix(t, 2.5 * sigma, output_type="coo_matrix")
    w = np.exp(-(D.data / sigma) ** 2)
    G = sp.coo_matrix((w, (D.row, D.col)), shape=D.shape).tocsr() + sp.identity(len(V), format="csr")
    return sp.diags(1 / np.asarray(G.sum(1)).ravel()) @ G


class Mapper:
    """Two-band delta transfer: the smooth (large-scale) part of a target follows a
    spatially smoothed gradient, its detail follows the local gradient.  A target that
    rigidly moves a whole hand therefore stays rigid, while eyelid closure still spans
    the much larger cartoon eye."""

    def __init__(self, J, G):
        self.J = J; self.G = G
        self.Js = (G @ J.reshape(len(J), 9)).reshape(-1, 3, 3)

    def __call__(self, idx, d):
        dense = np.zeros((len(self.J), 3)); dense[idx] = d
        low = (self.G @ dense)[idx]
        return np.einsum("nij,nj->ni", self.Js[idx], low) + np.einsum("nij,nj->ni", self.J[idx], d - low)


HEADER = ("# Retargeted for the cartoon hm08 bases (tools/cartoon_hm08).\n"
          "# Deltas are the MakeHuman originals carried through the per-vertex deformation\n"
          "# gradient of the hm08 -> cartoon fit.  Vertex numbering is unchanged hm08.\n")


def write_target(path, idx, d, original_header, compress):
    keep = np.abs(d).max(1) > 5e-6
    lines = "".join(f"{i} {x:.5f} {y:.5f} {z:.5f}\n" for i, (x, y, z) in zip(idx[keep], d[keep]))
    text = original_header + HEADER + lines
    path.parent.mkdir(parents=True, exist_ok=True)
    if compress:
        with gzip.open(str(path) + ".gz", "wt", compresslevel=6) as f:
            f.write(text)
    else:
        path.write_text(text)


def header_of(path):
    out = []
    for ln in Path(path).read_text().splitlines():
        if ln.startswith("#"):
            out.append(ln)
        else:
            break
    return "\n".join(out) + "\n" if out else ""
