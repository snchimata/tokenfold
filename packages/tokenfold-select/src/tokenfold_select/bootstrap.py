"""Direct-native entrypoint for an approved caller-owned Python environment."""
import argparse
from pathlib import Path
import sys


def main():
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--site-packages', type=Path, required=True)
    args, remaining = parser.parse_known_args()
    if (not sys.flags.isolated or not sys.flags.no_site
            or not args.site_packages.is_absolute() or not args.site_packages.is_dir()):
        raise ValueError('isolated_native_caller_environment_required')
    # No .pth processing, checkout imports, inherited PYTHONPATH or shell/shim.
    sys.path[:0] = [str(Path(__file__).resolve().parents[1]), str(args.site_packages.resolve())]
    from tokenfold_select.__main__ import main as run
    return run(remaining)


if __name__ == '__main__':
    raise SystemExit(main())
