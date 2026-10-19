"""Generate the reduced-kernel union from all six pinned ONNX graphs."""
from collections import defaultdict
import json
from pathlib import Path
import onnx

ROOT = Path(__file__).resolve().parent
operators = defaultdict(set)


def visit(graph, versions):
    for node in graph.node:
        domain = node.domain or 'ai.onnx'
        operators[domain, versions[domain]].add(node.op_type)
        for attribute in node.attribute:
            if attribute.type == onnx.AttributeProto.GRAPH:
                visit(attribute.g, versions)
            elif attribute.type == onnx.AttributeProto.GRAPHS:
                for child in attribute.graphs:
                    visit(child, versions)


for entry in json.loads((ROOT / 'models.json').read_text(encoding='utf-8')):
    if not entry['path'].endswith('.onnx'):
        continue
    model = onnx.load(ROOT / '.cache/models' / entry['path'])
    versions = {(op.domain or 'ai.onnx'): op.version for op in model.opset_import}
    visit(model.graph, versions)

# ORT 1.31 level-3 CPU graph fusions introduce kernels absent from source ONNX.
# These were enumerated from all six optimized graphs (same pinned ORT version).
operators['com.microsoft', 1].update({'FusedMatMul', 'Gelu', 'QuickGelu', 'SkipLayerNormalization'})
operators['com.microsoft.nchwc', 1].update({'AveragePool', 'Conv', 'GlobalAveragePool', 'MaxPool', 'ReorderInput', 'ReorderOutput', 'Upsample'})
operators['ai.onnx', 11].update({'LayerNormalization', 'SimplifiedLayerNormalization'})
text = '# Union of the six SHA-256-pinned PP-OCRv6 ONNX graphs in models.json.\n'
text += ''.join(f'{domain};{version};{",".join(sorted(names))}\n'
                for (domain, version), names in sorted(operators.items()))
(ROOT / 'required_operators.config').write_text(text, encoding='utf-8', newline='\n')
print(text)
