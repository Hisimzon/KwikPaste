"""按状态精确比较客户区像素，失败时写出醒目的差异图。"""
import argparse
import json
from pathlib import Path
from PIL import Image, ImageChops, ImageDraw


def compare(baseline: Path, candidate: Path, output: Path) -> int:
    """要求清单、尺寸和像素一致；遮罩必须由两边清单明确声明。"""
    output.mkdir(parents=True, exist_ok=True)
    left = {item["file"]: item for item in json.loads((baseline / "manifest.json").read_text())}
    right = {item["file"]: item for item in json.loads((candidate / "manifest.json").read_text())}
    results = []
    for name in sorted(left.keys() | right.keys()):
        record = {"state": name, "different_pixels": None, "masks": []}
        if name not in left or name not in right:
            record["error"] = "missing state"
        else:
            a, b = Image.open(baseline / name).convert("RGB"), Image.open(candidate / name).convert("RGB")
            if a.size != b.size:
                record["error"] = f"size mismatch: {a.size} != {b.size}"
            elif left[name]["masks"] != right[name]["masks"]:
                record["error"] = "mask mismatch"
            else:
                masks = left[name]["masks"]
                record["masks"] = masks
                for mask in masks:
                    ImageDraw.Draw(a).rectangle(mask["rect"], fill="black")
                    ImageDraw.Draw(b).rectangle(mask["rect"], fill="black")
                delta = ImageChops.difference(a, b)
                changed = delta.convert("RGB").point(lambda value: 255 if value else 0)
                count = sum(pixel != (0, 0, 0) for pixel in changed.getdata())
                record["different_pixels"] = count
                if count:
                    changed.save(output / name)
        results.append(record)
        print(f"{name}: {record.get('error', record['different_pixels'])}")
    failed = sum(row["different_pixels"] != 0 for row in results)
    report = {"baseline": str(baseline.resolve()), "candidate": str(candidate.resolve()), "states": results, "failed_states": failed}
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"states={len(results)} failed={failed}")
    return int(failed > 0)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    raise SystemExit(compare(args.baseline, args.candidate, args.output))
