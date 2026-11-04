"""Fetch additional CC0 morph data from the pinned MakeHuman revision."""
import concurrent.futures
import hashlib
import json
from pathlib import Path
import urllib.request

root = Path(__file__).resolve().parents[1] / 'assets' / 'makehuman'
manifest = json.loads((root / 'manifest.json').read_text())
commit = manifest['source_commit']
controls = [
    ('head_width', 'Head width', 'Face', 'head/head-scale-horiz'),
    ('head_height', 'Head height', 'Face', 'head/head-scale-vert'),
    ('head_depth', 'Head depth', 'Face', 'head/head-scale-depth'),
    ('neck_width', 'Neck width', 'Body', 'neck/neck-scale-horiz'),
    ('neck_height', 'Neck height', 'Body', 'neck/neck-scale-vert'),
    ('nose_depth', 'Nose depth', 'Face', 'nose/nose-scale-depth'),
    ('nose_height', 'Nose height', 'Face', 'nose/nose-scale-vert'),
    ('mouth_height', 'Mouth height', 'Face', 'mouth/mouth-scale-vert'),
    ('left_eye_size', 'Left eye opening', 'Face', 'eyes/l-eye-scale'),
    ('right_eye_size', 'Right eye opening', 'Face', 'eyes/r-eye-scale'),
    ('left_ear_size', 'Left ear size', 'Face', 'ears/l-ear-scale'),
    ('right_ear_size', 'Right ear size', 'Face', 'ears/r-ear-scale'),
    ('left_hand_size', 'Left hand size', 'Limbs', 'armslegs/l-hand-scale'),
    ('right_hand_size', 'Right hand size', 'Limbs', 'armslegs/r-hand-scale'),
    ('left_foot_size', 'Left foot size', 'Limbs', 'armslegs/l-foot-scale'),
    ('right_foot_size', 'Right foot size', 'Limbs', 'armslegs/r-foot-scale'),
]
for ident, label, category, stem in controls:
    if any(t['id'] == ident for t in manifest['targets']):
        continue
    manifest['targets'].append(dict(id=ident, label=label, category=category,
        negative=f'data/targets/{stem}-decr.target', positive=f'data/targets/{stem}-incr.target'))

def fetch(path):
    destination = root / path
    if not destination.exists():
        data = urllib.request.urlopen(f'https://raw.githubusercontent.com/makehumancommunity/makehuman/{commit}/makehuman/{path}', timeout=30).read()
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)

paths = [t[direction] for t in manifest['targets'] for direction in ('negative', 'positive')]
with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
    list(pool.map(fetch, paths))
(root / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
provenance = root / 'PROVENANCE.md'
text = provenance.read_text().split('## SHA-256')[0]
text += f'## Controls\n\n{len(manifest["targets"])} paired morph controls; 48 upstream target files. See manifest.json for labels and paths.\n\n## SHA-256\n\n'
for file in sorted(root.rglob('*')):
    if file.is_file() and file != provenance:
        text += f'- `{file.relative_to(root)}`: `{hashlib.sha256(file.read_bytes()).hexdigest()}`\n'
provenance.write_text(text)
print(f'{len(manifest["targets"])} controls available')
