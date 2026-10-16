"""Build/cache Microsoft's pinned CPU-only /MT ORT; no DLLs or minimal/ORT-only format.

Run with a Python environment containing cmake and ninja, in a Visual C++ developer
environment. x64 is the default; arm64 requires the corresponding installed toolset.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parent
VERSION = '1.31.0'
COMMIT = '402979ac1bef89dcd55add41eaa7277c407599eb'


def run(command, **kwargs):
    print(subprocess.list2cmdline([str(x) for x in command]), flush=True)
    return subprocess.run(command, check=True, **kwargs)


def mirror_dependencies(source):
    """Cache the pinned CPU dependencies using verified upstream bytes, not CMake TLS."""
    needed = {'abseil_cpp', 're2', 'protoc_win64', 'protobuf', 'date', 'mp11', 'json',
              'pytorch_cpuinfo', 'microsoft_gsl', 'microsoft_wil', 'safeint', 'flatbuffers', 'onnx',
              'eigen', 'dlpack', 'cxxopts', 'psimd', 'fp16'}
    mirror = ROOT / '.cache/deps'
    for line in (source / 'cmake/deps.txt').read_text(encoding='utf-8').splitlines():
        if not line or line.startswith('#'):
            continue
        name, url, sha = line.split(';')
        if name not in needed:
            continue
        path = mirror / url.removeprefix('https://')
        if path.is_file() and hashlib.sha1(path.read_bytes()).hexdigest() == sha:
            continue
        print('Cache dependency', name, url, flush=True)
        with urllib.request.urlopen(url, timeout=90) as response:
            data = response.read()
        if hashlib.sha1(data).hexdigest() != sha:
            raise RuntimeError('Upstream dependency checksum mismatch: ' + name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    return mirror


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--target', choices=['x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc'], default='x86_64-pc-windows-msvc')
    args = parser.parse_args()
    if sys.platform != 'win32':
        raise RuntimeError('This recipe only builds Windows extensions')
    ops = ROOT / 'required_operators.config'
    key = hashlib.sha256(ops.read_bytes()).hexdigest()[:12]
    build = ROOT / f'.cache/ort-{VERSION}-{args.target}-{key}'
    stamp = build / 'complete.json'
    recipe_sha = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    if stamp.is_file() and (build / 'Release/onnxruntime.lib').is_file() and json.loads(stamp.read_text()).get('recipe_sha256') == recipe_sha:
        print('Cached ORT:', build / 'Release')
        return
    source = ROOT / '.cache/onnxruntime-src'
    if not source.exists():
        run(['git', '-c', 'core.longpaths=true', 'clone', '--depth', '1', '--branch', 'v' + VERSION,
             'https://github.com/microsoft/onnxruntime.git', source])
    actual = subprocess.check_output(['git', '-C', source, 'rev-parse', 'HEAD'], text=True).strip()
    if actual != COMMIT:
        raise RuntimeError('Unexpected ORT checkout: ' + actual)
    mirror = mirror_dependencies(source)
    cmake = shutil.which('cmake')
    if not cmake or not shutil.which('ninja'):
        raise RuntimeError('Install cmake >=3.28 and ninja in the active Python environment')
    command = [sys.executable, source / 'tools/ci_build/build.py', '--config', 'Release', '--update', '--build',
               '--build_dir', build, '--parallel', '8', '--skip_tests', '--skip_submodule_sync',
               '--compile_no_warning_as_error', '--enable_msvc_static_runtime', '--cmake_generator', 'Ninja',
               '--cmake_deps_mirror_dir', mirror,
               '--cmake_path', cmake, '--include_ops_by_config', ops, '--disable_ml_ops', '--disable_generation_ops',
               '--cmake_extra_defines', 'onnxruntime_BUILD_UNIT_TESTS=OFF', 'onnxruntime_BUILD_SHARED_LIB=OFF',
               'onnxruntime_USE_DML=OFF', 'onnxruntime_USE_CUDA=OFF', 'onnxruntime_USE_TENSORRT=OFF',
               'onnxruntime_USE_TELEMETRY=OFF', 'onnxruntime_USE_MIMALLOC=OFF', 'CMAKE_OBJECT_PATH_MAX=160']
    # ORT's dependency patches fail with Strawberry Perl's old patch.exe, which comes first on CI's PATH.
    git_patch = Path(shutil.which('git')).parents[1] / 'usr/bin/patch.exe'
    if git_patch.is_file():
        command += ['Patch_EXECUTABLE=' + git_patch.as_posix()]
    # Ninja uses the loaded MSVC target environment; it must not receive --arm64.
    if args.target.startswith('aarch64'):
        if os.environ.get('VSCMD_ARG_TGT_ARCH', '').lower() != 'arm64':
            raise RuntimeError('Load the x64_arm64 MSVC toolset first')
        command += ['CMAKE_SYSTEM_PROCESSOR=ARM64']
    run(command, cwd=source)
    if not (build / 'Release/onnxruntime_session.lib').is_file():
        raise RuntimeError('ORT static session library was not produced')
    # ort-sys' split-library layout assumes re2 even when reduced ORT omits it.
    # Merge only built /MT archives, retaining normal linker dead-code elimination.
    release = build / 'Release'
    libraries = sorted(release.glob('onnxruntime_*.lib'))
    libraries = [p for p in libraries if p.name != 'onnxruntime_providers_shared.lib']
    libraries += sorted((release / '_deps').rglob('*.lib'))
    libraries += sorted((release / 'model_package').glob('*.lib'))
    libraries = [p for p in libraries if p.name not in {'libprotobuf.lib', 'libprotoc.lib', 'libupb.lib'}]
    response = build / 'merge.rsp'
    response.write_text('\n'.join('"' + str(p) + '"' for p in libraries), encoding='utf-8')
    run(['lib.exe', '/NOLOGO', '/OUT:' + str(release / 'onnxruntime.lib'), '@' + str(response)])
    stamp.write_text(json.dumps(dict(version=VERSION, commit=COMMIT, target=args.target, recipe_sha256=recipe_sha,
                                    ops_sha256=hashlib.sha256(ops.read_bytes()).hexdigest(),
                                    flags=[str(x) for x in command]), indent=2), encoding='utf-8')
    print('ORT_LIB_LOCATION=' + str(build / 'Release'))


if __name__ == '__main__':
    main()
