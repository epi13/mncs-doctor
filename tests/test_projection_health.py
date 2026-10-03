"""Native diagnosis and safe targeted repair, without a host verdict mirror."""
import json
import os
import subprocess
from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get('MNCS_BIN') or ROOT.parent / 'mncs-language/target/release/mncs'
pytestmark = pytest.mark.skipif(not Path(BINARY).is_file(), reason='MNCS compiler unavailable')


def call(function, facts):
    result = subprocess.run([str(BINARY), 'call', str(ROOT / 'mncs/doctor/projection.mncs'),
        '--module', 'doctor.projection.v1', '--function', function, '--args-json',
        json.dumps([{'integer': {'value': value}} for value in facts])],
        capture_output=True, text=True, timeout=30, check=True)
    document = json.loads(result.stdout)
    assert document['status'] == 'returned'
    return document['call']['returned'][0]['integer']['value']


@pytest.mark.parametrize('index,value,expected', [(None,None,0),(6,0,1),(1,0,2),
    (5,0,3),(0,0,4),(2,0,5),(4,0,6),(3,0,7),(7,0,8),(8,1,9)])
def test_health_distinctions(index, value, expected):
    facts = [1,1,1,1,1,1,1,1,0]
    if index is not None:
        facts[index] = value
    assert call('health', facts) == expected


@pytest.mark.parametrize('health,owner,claim,confined,expected', [
    (0,1,1,1,0),(1,1,1,1,1),(2,1,1,1,1),(3,1,1,1,1),
    (1,0,1,1,2),(1,1,0,1,2),(1,1,1,0,2),(5,1,1,1,2),(6,1,1,1,2),(7,1,1,1,2),(8,1,1,1,2)])
def test_repair_admission(health, owner, claim, confined, expected):
    assert call('repair', [health, owner, claim, confined]) == expected


def test_structure_preserves_unknown_and_missing_failure():
    assert call('structure_conformance',[1,0,0]) == 1
    assert call('structure_conformance',[0,0,0]) == 2
    assert call('structure_conformance',[1,1,0]) == 0
