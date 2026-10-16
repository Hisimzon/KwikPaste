"""Reuse the unchanged 41-image prototype benchmark; never write to its directory."""
import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--prototype', type=Path, required=True)
    parser.add_argument('--binaries', type=Path, default=ROOT / 'target/release')
    parser.add_argument('--label', default='final')
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location('prototype_bench', args.prototype / 'bench.py')
    bench = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(bench)
    manifest_file = args.prototype / 'dataset/manifest.json'
    manifest = json.loads(manifest_file.read_text(encoding='utf-8'))
    assert len(manifest) == 41
    for item in manifest:
        image = args.prototype / 'dataset' / item['image']
        assert hashlib.sha256(image.read_bytes()).hexdigest() == item['image_sha256']
    output = ROOT / 'results'
    output.mkdir(exist_ok=True)
    for repeat in range(1, args.repeats + 1):
        tiers = ['tiny', 'small', 'medium']
        if repeat % 2 == 0:
            tiers.reverse()
        for tier in tiers:
            exe = args.binaries.resolve() / f'kwikpaste-ext-ocr-{tier}.exe'
            run = bench.run_engine(tier, exe, manifest, repeat)
            assert run['exit_code'] == 0 and run['monitor_error'] is None
            parity = {}
            for name, filename in [
                ('prototype', f'tier-parity-{tier}-1.json'),
                ('official', 'reference.json' if tier == 'small' else f'reference-{tier}.json'),
            ]:
                reference = json.loads((args.prototype / 'results' / filename).read_text(encoding='utf-8'))
                previous = {row['id']: row['text'] for row in reference['images']}
                edits = chars = 0
                worst = 0
                lines_equal = True
                for row in run['images']:
                    old = previous[row['id']]
                    distance = bench.distance(bench.normalized(old), bench.normalized(row['text']))
                    length = max(1, len(bench.normalized(old)))
                    edits += distance
                    chars += length
                    worst = max(worst, distance / length)
                    lines_equal &= len(old.splitlines()) == len(row['text'].splitlines())
                parity[name] = dict(edits=edits, chars=chars, divergence=edits / chars,
                                    worst=worst, lines_equal=lines_equal)
                assert edits / chars <= .01 and worst <= .10 and lines_equal, parity
            run['parity'] = parity
            run['manifest_sha256'] = hashlib.sha256(manifest_file.read_bytes()).hexdigest()
            (output / f'{args.label}-{tier}-{repeat}.json').write_text(
                json.dumps(run, ensure_ascii=False, indent=2), encoding='utf-8')
            print('PARITY', tier, parity, flush=True)


if __name__ == '__main__':
    main()
