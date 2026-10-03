"""Doctor-owned projection health/repair transport through Forge artifacts."""
from __future__ import annotations
import argparse
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAMES = ('current', 'stale', 'missing', 'renderer-outdated', 'source-unavailable',
         'target-occupied', 'manual-divergence', 'malformed-mixed-projection',
         'failed-validation', 'blocked-repair')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--facts-json', required=True)
    args = parser.parse_args(argv)
    family = Path(os.environ.get('MNCS_WORKSPACE_ROOT', ROOT.parent))
    sys.path.insert(0, str(family / 'mncs-forge/src'))
    from mncs_forge.provider_artifacts import ProviderArtifact, integer
    spec = json.loads((ROOT / '.mncs/projection-artifact.json').read_text())
    compiler = Path(os.environ.get('MNCS_BIN', str(family / 'mncs-language/target/release/mncs')))
    cache = Path(os.environ.get('MNCS_PROVIDER_CACHE', str(Path.home() / '.cache/mncs-doctor/projections')))
    provider = ProviderArtifact(spec, roots={'mncs-doctor': ROOT, 'MNCS-Commons': family / 'MNCS-Commons'},
        compiler=compiler, embed=compiler.parent / 'libmncs_embed.so', cache=cache)
    request = json.loads(args.facts_json)
    try:
        if 'structure' in request:
            result, receipt = provider.call('doctor.projection.v1', 'structure_conformance',
                [integer(v) for v in request['structure']])
            verdict = result['returned'][0]['integer']['value']
            print(json.dumps({'schema_version': 'mncs.projection-structure/1',
                'state': {0: 'fail', 1: 'pass', 2: 'unknown'}[verdict], 'provenance': receipt['identity']}))
            return 0
        result, receipt = provider.call('doctor.projection.v1', 'health', [integer(v) for v in request['health']])
        status = result['returned'][0]['integer']['value']
        result, repair_receipt = provider.call('doctor.projection.v1', 'repair',
            [integer(status)] + [integer(v) for v in request['repair']])
        decision = result['returned'][0]['integer']['value']
        print(json.dumps({'schema_version': 'mncs.projection-health/1', 'status': NAMES[status],
                         'code': status, 'repair': decision, 'provenance': [receipt, repair_receipt]}))
    finally:
        provider.close()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
