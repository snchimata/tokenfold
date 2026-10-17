"""Inspection or the existing one-shot Select file protocol; no downloads."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import threading

from .artifact import inspect_artifact
from .protocol import REQUEST_BYTES, encode_response, load_request
from .runtime import DevelopmentScorer
from .onnx import DEFAULT_MANIFEST_SHA256, DEFAULT_REVISION, OnnxScorer, inspect_onnx


def configure(args):
    """Generate approval only on explicit request; never download or invoke a model."""
    model = inspect_onnx(args.artifact, args.manifest_sha256)
    if model['source_checkpoint_sha256'] != args.revision:
        raise ValueError('model_revision_mismatch')
    if (not args.approved_by.strip() or not args.scratch_root.is_absolute()
            or not args.scratch_root.is_dir() or not args.site_packages.is_absolute()
            or not args.site_packages.is_dir()):
        raise ValueError('explicit_approval_and_absolute_existing_directories_required')
    executable = Path(sys._base_executable).resolve(strict=True)
    if executable.stat().st_size > 64 * 1024 * 1024:
        raise ValueError('executable_byte_limit')
    bootstrap = Path(__file__).with_name('bootstrap.py').resolve()
    with executable.open('rb') as stream:
        pin = hashlib.file_digest(stream, 'sha256').hexdigest()
    return dict(executable=str(executable), executable_sha256=pin,
                arguments=['-I', '-S', str(bootstrap), '--site-packages', str(args.site_packages.resolve()),
                           'worker', '--artifact', str(args.artifact.resolve()), '--revision', args.revision,
                           '--manifest-sha256', args.manifest_sha256, '--development'],
                model_revision=args.revision, approved_by=args.approved_by,
                scratch_root=str(args.scratch_root.resolve()))


def _worker(args):
    # A native direct child is required. This timer never starts on import/inspection.
    timer = threading.Timer(30, lambda: os._exit(124))
    timer.daemon = True
    timer.start()
    code = 1
    try:
        # Set before importing any inference library. ONNX is explicitly CPU-only.
        if args.backend == 'onnx':
            os.environ['CUDA_VISIBLE_DEVICES'] = '-1'
            os.environ['TOKENIZERS_PARALLELISM'] = 'false'
            if os.name == 'nt':
                # The Rust parent clears the environment. Obtain the system path
                # from Windows, not an inherited/user-controlled environment value.
                import ctypes
                system = ctypes.create_unicode_buffer(32768)
                length = ctypes.windll.kernel32.GetWindowsDirectoryW(system, len(system))
                if not 0 < length < len(system):
                    raise ValueError('windows_system_directory_unavailable')
                os.environ['SystemRoot'] = system.value
        request_path = Path(os.environ['TOKENFOLD_SELECT_REQUEST_PATH'])
        response_path = Path(os.environ['TOKENFOLD_SELECT_RESPONSE_PATH'])
        if (not request_path.is_absolute() or not response_path.is_absolute()
                or request_path.resolve().parent != response_path.resolve().parent
                or request_path.resolve() == response_path.resolve() or response_path.exists()):
            raise ValueError('caller_owned_request_response_paths_required')
        def offline(event, _):
            if event in ('socket.connect', 'socket.getaddrinfo', 'socket.bind', 'subprocess.Popen', 'os.system'):
                raise ValueError('offline_direct_child_only')
        sys.addaudithook(offline)
        with request_path.open('rb') as stream:
            request = load_request(stream.read(REQUEST_BYTES + 1), args.revision)
        if args.backend == 'onnx':
            scorer = OnnxScorer(args.artifact, args.manifest_sha256, development=args.development)
            if scorer.revision != args.revision:
                raise ValueError('model_revision_mismatch')
        else:
            scorer = DevelopmentScorer(args.artifact, args.revision, cache_directory=request_path.parent / 'model-cache',
                                       development=args.development, precision='fp32')
        response = scorer.score(request)
        raw = encode_response(request, [value for _, value in response['scores']])
        with response_path.open('xb') as stream:
            stream.write(raw)
        if args.backend == 'pytorch':
            scorer._torch.cuda.synchronize()
        code = 0
    except BaseException:
        print('tokenfold-select: refused', file=sys.stderr, flush=True)
    finally:
        # Avoid potentially unbounded framework teardown; native parent owns kill/wait.
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(code)


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for command in ('inspect', 'inspect-onnx', 'configure', 'worker'):
        mode = sub.add_parser(command)
        mode.add_argument('--artifact', type=Path, required=True)
        mode.add_argument('--revision', default=DEFAULT_REVISION)
        mode.add_argument('--manifest-sha256', default=DEFAULT_MANIFEST_SHA256)
        if command == 'worker':
            mode.add_argument('--backend', choices=('onnx', 'pytorch'), default='onnx')
            mode.add_argument('--development', action='store_true', required=True)
        if command == 'configure':
            mode.add_argument('--approved-by', required=True)
            mode.add_argument('--site-packages', type=Path, required=True)
            mode.add_argument('--scratch-root', type=Path, required=True)
    args = parser.parse_args(arguments)
    if args.command == 'worker':
        _worker(args)
    try:
        if args.command == 'configure':
            print(json.dumps(configure(args)))
            return 0
        if args.command == 'inspect-onnx':
            report = inspect_onnx(args.artifact, args.manifest_sha256)
            if report['source_checkpoint_sha256'] != args.revision:
                raise ValueError('model_revision_mismatch')
            report = dict(checkpoint_sha256=report['source_checkpoint_sha256'])
        else:
            report = inspect_artifact(args.artifact, args.revision)
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        print('tokenfold-select: refused', file=sys.stderr)
        return 1
    print(json.dumps(dict(verification='metadata_and_hashes_only', checkpoint_tensors_loaded=False,
                          model_revision=report['checkpoint_sha256'], development_only=True,
                          qualification=False, release_approved=False, redistribution_cleared=False)))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
