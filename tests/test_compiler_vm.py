import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1];WS=ROOT.parent
spec=importlib.util.spec_from_file_location('doctor_composition',ROOT/'tools/compiler_vm.py');doctor=importlib.util.module_from_spec(spec);spec.loader.exec_module(doctor)
COMPILER=WS/'mncs-compiler';EXE=COMPILER/'.bootstrap/target/release/mncs-compiler-stage0-probe';VM=WS/'mncs-vm/target/debug/mncs-vm';LANGUAGE=WS/'mncs-language/target/release/mncs'

@unittest.skipUnless(EXE.is_file() and VM.is_file() and LANGUAGE.is_file(),'exact providers not built')
class CompositionHealthTests(unittest.TestCase):
    def test_compact_provenance_keeps_cross_boundary_identities(self):
        vm_receipt = {'identity': 'vm-build-1', 'receipt': {'source_inputs': {'a': 'sha256:a'}}}
        compiler_receipt = {'identity': 'compiler-build-1'}
        value = {
            'schema_version': 'execution/1', 'identity': 'execution-1',
            'core': {
                'operation': 'call', 'provider': 'mncs-vm',
                'artifact_identity': 'artifact-1', 'executor_sha256': 'vm-exe-1',
                'build_receipt': 'call-receipt-1',
                'abi': {'build_origin': compiler_receipt},
                'runtime': {
                    'build_origin': {'status': 'matches-embedded-inputs'},
                    'contract': {'artifact_schema': 'mncs.vm.artifact/1',
                                 'vm_contract': 'mncs.vm/0.1',
                                 'build_origin': vm_receipt},
                    'executable': '/selected/mncs-vm', 'executable_sha256': 'vm-exe-1',
                    'schema_version': 'mncs.vm.selected-runtime/1',
                },
            },
        }
        compact = doctor.compact_execution_provenance(value)
        self.assertEqual(compact['producer_identity'], 'compiler-build-1')
        self.assertEqual(compact['runtime']['identity'], 'vm-build-1')
        self.assertEqual(compact['runtime']['artifact_schema'], 'mncs.vm.artifact/1')
        self.assertEqual(compact['core']['artifact_identity'], 'artifact-1')
        self.assertLess(len(json.dumps(compact)), len(json.dumps(value)))

    def test_unknown_without_proof_pass_with_bounded_proof_stale_refused(self):
        args=dict(compiler_checkout=COMPILER,compiler_executable=EXE,vm_checkout=WS/'mncs-vm',vm_executable=VM,stage0=LANGUAGE)
        self.assertEqual(doctor.diagnose(**args)['status'],'unknown')
        spec=importlib.util.spec_from_file_location('health_selected_compiler',COMPILER/'tools/vm_provider.py');producer=importlib.util.module_from_spec(spec);spec.loader.exec_module(producer)
        with tempfile.TemporaryDirectory() as raw:
            root=Path(raw);source=root/'source.mncs';source.write_text('mncs 0.18; module proof.doctor.v1; fn value() -> (result: i64) { return 7; }')
            product=producer.CompilerProvider(COMPILER).emit({'schema_version':'mncs.compiler-vm-request/1','source':str(source),'logical_name':'source.mncs'},root/'cache')
            request=root/'request.json';request.write_text(json.dumps({'schema_version':'0.1','target':{'module':'proof.doctor.v1','function':'value'},'arguments':[],'step_budget':100}))
            proof=doctor.diagnose(**args,product_path=product['product'],request_path=request)
            self.assertEqual(proof['status'],'pass');self.assertEqual(proof['compatibility']['admission'],'admitted')
            self.assertEqual(proof['optional_backends']['portable_wasm'],'not_probed')
            source.write_text(source.read_text().replace('return 7','return 8'))
            stale=doctor.diagnose(**args,product_path=product['product'],request_path=request)
            self.assertEqual(stale['status'],'fail');self.assertIn('stale',stale['reason'])
            path=Path(product['product']);modified=json.loads(path.read_text());modified['artifact']['identity']='forged';path.write_text(json.dumps(modified))
            self.assertEqual(doctor.diagnose(**args,product_path=path,request_path=request)['status'],'fail')
