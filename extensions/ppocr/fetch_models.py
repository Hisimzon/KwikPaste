"""Fetch the pinned official ONNX/YAML files, validating every cached byte."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import urllib.request

ROOT = Path(__file__).resolve().parent


def fetch(seed=None):
    for entry in json.loads((ROOT / 'models.json').read_text(encoding='utf-8')):
        path = ROOT / '.cache/models' / entry['path']
        if path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == entry['sha256']:
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        cached = Path(seed) / entry['prototype_path'] if seed else None
        if cached and cached.is_file() and hashlib.sha256(cached.read_bytes()).hexdigest() == entry['sha256']:
            shutil.copyfile(cached, path)
            continue
        print('Download', entry['url'], flush=True)
        with urllib.request.urlopen(entry['url'], timeout=180) as response:
            data = response.read()
        if len(data) != entry['bytes'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise RuntimeError('Model checksum mismatch: ' + entry['path'])
        temporary = path.with_suffix(path.suffix + '.tmp')
        temporary.write_bytes(data)
        temporary.replace(path)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--prototype-cache')
    fetch(parser.parse_args().prototype_cache)
