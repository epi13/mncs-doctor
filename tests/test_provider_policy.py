import json
import os
import subprocess
from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('MNCS_BINARY', '/unavailable'))

@pytest.mark.parametrize('facts,expected', [([1,1,1,1,0],0),([2,1,1,1,0],1),([2,0,1,1,0],2),([2,1,0,1,0],2),([2,1,1,0,0],2),([2,1,1,1,1],2),([0,1,1,1,0],3)])
def test_exact_target_provider_repair_policy(facts,expected):
    if not BINARY.is_file(): pytest.skip('explicit compiler required')
    result=subprocess.run([str(BINARY),'call',str(ROOT/'mncs/doctor/provider.mncs'),'--module','doctor.provider.v1','--function','repair_artifact','--args-json',json.dumps([{'integer':{'value':v}} for v in facts])],capture_output=True,text=True,timeout=30)
    assert result.returncode==0,result.stdout+result.stderr
    assert json.loads(result.stdout)['call']['returned'][0]['integer']['value']==expected


@pytest.mark.parametrize('facts,expected', [([1,1,1,1,0,0,1],'proceed'),([1,1,1,1,1,0,1],'escalate'),([1,0,1,1,0,0,1],'defer')])
def test_doctor_family_entrypoint_preserves_shared_law(facts,expected):
    if not BINARY.is_file(): pytest.skip('explicit compiler required')
    commons = Path(os.environ.get('MNCS_COMMONS_ROOT', ROOT.parent / 'MNCS-Commons'))
    result=subprocess.run([str(BINARY),'call',str(ROOT/'mncs/doctor/family_change.mncs'),'--module','doctor.family_change.v1','--function','gate_repair','--args-json',json.dumps([{'integer':{'value':v}} for v in facts]),'--library',str(commons/'src/mncs_commons/mesh')],capture_output=True,text=True,timeout=30)
    assert result.returncode==0,result.stdout+result.stderr
    assert json.loads(result.stdout)['call']['returned'][0]['finite']['variant_identity']=='mncs:0.2:finite-variant:mncs.commons.family.change.v1::Gate::'+expected
