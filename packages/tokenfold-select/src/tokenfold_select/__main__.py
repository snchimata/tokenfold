"""Inspection or the existing one-shot Select file protocol; no downloads."""
import argparse
import json
import os
from pathlib import Path
import sys
import threading

from .artifact import inspect_artifact
from .protocol import REQUEST_BYTES, encode_response, load_request
from .runtime import DevelopmentScorer


def _worker(args):
    # A native direct child is required. This timer never starts on import/inspection.
    timer = threading.Timer(30, lambda: os._exit(124))
    timer.daemon = True
    timer.start()
    code = 1
    try:
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
        scorer = DevelopmentScorer(args.artifact, args.revision, cache_directory=request_path.parent / 'model-cache',
                                   development=args.development, precision='fp32')
        response = scorer.score(request)
        raw = encode_response(request, [value for _, value in response['scores']])
        with response_path.open('xb') as stream:
            stream.write(raw)
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
    for command in ('inspect', 'worker'):
        mode = sub.add_parser(command)
        mode.add_argument('--artifact', type=Path, required=True)
        mode.add_argument('--revision', required=True)
        if command == 'worker':
            mode.add_argument('--development', action='store_true', required=True)
    args = parser.parse_args(arguments)
    if args.command == 'worker':
        _worker(args)
    try:
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
