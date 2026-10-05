#!/usr/bin/env python3
"""Doctor's read-only diagnosis of an explicitly selected compiler/VM product.

Compiler owns producer inspection; VM owns standalone admission and fuel. A
Doctor PASS here proves this bounded call, not optional native/WASM backends.
"""
from __future__ import annotations
import argparse
import importlib.util
import json
import os
import subprocess
import sys
import hashlib
from pathlib import Path


def diagnose(*, compiler_checkout, compiler_executable, vm_checkout, vm_executable, stage0, product_path=None, request_path=None, language_checkout=None):
    report = {'schema_version':'mncs.doctor.compiler-vm/1','status':'unknown','components':{},
              'optional_backends':{'cranelift_project_session':'not_probed','portable_wasm':'not_probed'}}
    path = Path(compiler_checkout).resolve() / 'tools/vm_provider.py'
    spec = importlib.util.spec_from_file_location('doctor_selected_compiler_provider', path)
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    producer = module.CompilerProvider(Path(compiler_checkout), executable=Path(compiler_executable)).inspect()
    report['components']['compiler'] = producer
    path = Path(vm_checkout).resolve() / 'python/mncs_vm_client/__init__.py'
    spec = importlib.util.spec_from_file_location('doctor_selected_vm_transport', path)
    client = importlib.util.module_from_spec(spec); spec.loader.exec_module(client)
    inspect_runtime, Session, sha, digest = client.inspect_runtime, client.Session, client.sha, client.digest
    runtime = inspect_runtime(Path(vm_executable))
    report['components']['runtime'] = runtime
    if runtime.get('build_origin', {}).get('status') != 'matches-embedded-inputs':
        report['status'] = 'fail'
        report['reason'] = 'selected VM executable has unknown or stale build-input evidence'
        return report
    reference = Path(stage0).resolve()
    reference_probe = subprocess.run([str(reference), 'language-inventory'], capture_output=True, timeout=10, check=False)
    language_checkout = Path(language_checkout).resolve() if language_checkout else reference.parents[2]
    language_provider_path = language_checkout / 'tools/runtime_provider.py'
    reference_origin = {'status': 'unknown', 'reason': 'selected reference has no verified local build receipt'}
    if language_provider_path.is_file():
        spec = importlib.util.spec_from_file_location('doctor_selected_reference_provider', language_provider_path)
        reference_provider = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(reference_provider)
        reference_origin = reference_provider.inspect(reference, expected_checkout=language_checkout).get('build_origin', reference_origin)
    report['components']['reference'] = {'executable':str(reference), 'executable_sha256':sha(reference),
        'callable':reference_probe.returncode==0, 'build_origin':reference_origin}
    compatible = (producer.get('artifact_schema') == runtime['contract']['artifact_schema']
        and producer.get('vm_contract') == runtime['contract']['vm_contract']
        and producer.get('ssa_schema') in runtime['contract']['ssa_schemas'])
    report['compatibility'] = {'declarations_agree':compatible, 'admission':'not_probed', 'execution':'not_probed'}
    if (producer['state'] != 'ready' or not compatible or reference_probe.returncode
            or reference_origin.get('status') != 'matches-embedded-inputs'):
        report['status'] = 'fail'; return report
    if product_path is None or request_path is None:
        return report
    product = json.loads(Path(product_path).read_text())
    receipt = product['build_receipt']
    if (receipt['identity'] != digest(receipt['core']) or product['artifact'] != receipt['core']['artifact']
            or product['evidence'] != receipt['core']['evidence'] or receipt['core']['producer'] != producer['producer']['identity']
            or receipt['core']['inputs']['compiler_artifact_sha256'] != producer['executable_sha256']):
        report['status'] = 'fail'; report['reason']='stale or incompatible compiler product'; return report
    cache = Path(product_path).resolve().parent
    artifact = (cache / product['artifact']['address']).resolve()
    evidence_path = (cache / product['evidence']['address']).resolve()
    if (not artifact.is_relative_to(cache) or not evidence_path.is_relative_to(cache)
            or sha(artifact) != product['artifact']['sha256'] or sha(evidence_path) != product['evidence']['sha256']):
        report['status']='fail'; report['reason']='corrupt product bytes'; return report
    evidence = json.loads(evidence_path.read_text())
    if any(not Path(p).is_file() or sha(p) != expected for p,expected in evidence['source_inputs'].items()):
        report['status']='fail'; report['reason']='stale product source closure'; return report
    request = json.loads(Path(request_path).read_text())
    with Session(Path(vm_executable), artifact, build_receipt=receipt) as session:
        if session.info['artifact_id'] != product['artifact']['identity']:
            raise RuntimeError('admitted artifact identity differs from product')
        result = session.call(request)
    report['compatibility'].update({'admission':'admitted','execution':result['outcome']['kind'],
        'artifact_identity':session.info['artifact_id'], 'execution_provenance':result['execution_provenance'],
        'returned':result['execution']['returned'], 'usage':result['record']['usage'], 'resource_envelope':result['record'].get('resource_limits')})
    report['status'] = 'pass' if result['outcome']['kind']=='completed' else 'fail'
    return report


def reconcile_selected_builds():
    """Use exact selected provider build operations for stale local binaries."""
    language_checkout = Path(os.environ['MNCS_LANGUAGE_ROOT']).resolve()
    reference_executable = Path(os.environ['MNCS_REFERENCE_BIN']).resolve()
    language_provider_path = language_checkout / 'tools/runtime_provider.py'
    spec = importlib.util.spec_from_file_location('doctor_build_selected_reference', language_provider_path)
    language_provider = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(language_provider)
    reference = language_provider.inspect(reference_executable, expected_checkout=language_checkout)
    repairs = []
    if reference.get('build_origin', {}).get('status') != 'matches-embedded-inputs':
        result = subprocess.run(
            [sys.executable, str(language_provider_path), 'build', '--executable', str(reference_executable)],
            cwd=language_checkout, capture_output=True, text=True, timeout=1800, check=False,
        )
        if result.returncode:
            raise RuntimeError('selected Stage-0/reference provider build failed: ' + result.stderr[-2000:])
        repairs.append({'provider': 'mncs-language', 'result': json.loads(result.stdout)})
    compiler_checkout = Path(os.environ['MNCS_COMPILER_CHECKOUT']).resolve()
    compiler_executable = Path(os.environ['MNCS_COMPILER_PROBE']).resolve()
    vm_checkout = Path(os.environ['MNCS_VM_CHECKOUT']).resolve()
    vm_executable = Path(os.environ['MNCS_VM_BIN']).resolve()
    compiler_module_path = compiler_checkout / 'tools/vm_provider.py'
    spec = importlib.util.spec_from_file_location('doctor_build_selected_compiler', compiler_module_path)
    compiler_provider = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(compiler_provider)
    producer = compiler_provider.CompilerProvider(compiler_checkout, executable=compiler_executable).inspect()
    if producer.get('state') != 'ready':
        result = subprocess.run(
            [sys.executable, str(compiler_module_path), 'build', '--executable', str(compiler_executable)],
            cwd=compiler_checkout, capture_output=True, text=True, timeout=1800, check=False,
        )
        if result.returncode:
            raise RuntimeError('selected compiler provider build failed: ' + result.stderr[-2000:])
        repairs.append({'provider': 'mncs-compiler', 'result': json.loads(result.stdout)})

    vm_provider_path = vm_checkout / 'tools/provider.py'
    try:
        vm_transport = vm_checkout / 'python/mncs_vm_client/__init__.py'
        vm_spec = importlib.util.spec_from_file_location('doctor_build_selected_vm_transport', vm_transport)
        vm_client = importlib.util.module_from_spec(vm_spec)
        vm_spec.loader.exec_module(vm_client)
        runtime = vm_client.inspect_runtime(vm_executable)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
        runtime = {'build_origin': {'status': 'unknown'}}
    if runtime.get('build_origin', {}).get('status') != 'matches-embedded-inputs':
        result = subprocess.run(
            [sys.executable, str(vm_provider_path), 'build', '--executable', str(vm_executable)],
            cwd=vm_checkout, capture_output=True, text=True, timeout=1800, check=False,
        )
        if result.returncode:
            raise RuntimeError('selected VM provider build failed: ' + result.stderr[-2000:])
        repairs.append({'provider': 'mncs-vm', 'result': json.loads(result.stdout)})
    return repairs


def compact_execution_provenance(value):
    """Keep cross-boundary identities without repeating both full VM contracts."""
    if not isinstance(value, dict):
        return value
    core = value.get('core') or {}
    abi = core.get('abi') or {}
    producer_origin = abi.get('build_origin') or {}
    runtime = core.get('runtime') or {}
    contract = runtime.get('contract') or {}
    runtime_origin = runtime.get('build_origin') or contract.get('build_origin') or {}
    if not runtime_origin.get('identity'):
        runtime_origin = contract.get('build_origin') or runtime_origin
    return {
        'schema_version': value.get('schema_version'),
        'identity': value.get('identity'),
        'core': {
            key: core.get(key) for key in (
                'operation', 'provider', 'artifact_identity', 'artifact_sha256',
                'executor_sha256', 'input_identity', 'result_identity',
                'subject_identity', 'inventory_identity', 'build_receipt',
                'request', 'envelope_override',
            ) if key in core
        },
        'producer_identity': producer_origin.get('identity'),
        'runtime': {
            'identity': runtime_origin.get('identity'),
            'executable': runtime.get('executable'),
            'executable_sha256': runtime.get('executable_sha256'),
            'artifact_schema': contract.get('artifact_schema'),
            'vm_contract': contract.get('vm_contract'),
            'schema_version': runtime.get('schema_version'),
        },
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--product')
    parser.add_argument('--request')
    parser.add_argument('--smoke',action='store_true',help='provider-owned bounded direct compiler/VM proof')
    parser.add_argument('--reconcile-builds',action='store_true',help='rebuild stale selected compiler/VM providers using their declared build operations')
    parser.add_argument('--cache',help='explicit smoke artifact cache; Environment supplies its session artifact root')
    parser.add_argument('--compact',action='store_true',help='summarize locally verified input hashes for bounded readiness transport')
    args = parser.parse_args(argv)
    try:
        if args.smoke:
            if args.product or args.request:
                parser.error('--smoke cannot be combined with a product/request proof')
            cache_raw = args.cache or os.environ.get('MNCS_ENV_SESSION_ARTIFACT_DIR')
            if not cache_raw:
                parser.error('--smoke needs --cache or an Environment session artifact root')
            repairs = reconcile_selected_builds() if args.reconcile_builds else []
            checkout=Path(os.environ['MNCS_COMPILER_CHECKOUT']).resolve()
            spec=importlib.util.spec_from_file_location('doctor_smoke_selected_producer',checkout/'tools/vm_provider.py')
            provider=importlib.util.module_from_spec(spec);spec.loader.exec_module(provider)
            source=Path(__file__).resolve().parent/'fixtures/compiler_vm_smoke.mncs'
            cache=Path(cache_raw).resolve()/'compiler-vm-smoke'
            product=provider.CompilerProvider(checkout,executable=Path(os.environ['MNCS_COMPILER_PROBE'])).emit(
                {'schema_version':'mncs.compiler-vm-request/1','source':str(source),'logical_name':source.name},cache)
            request={'schema_version':'0.1','target':{'module':'mncs.doctor.compiler_vm_smoke.v1','function':'proof'},'arguments':[],'step_budget':100}
            request_path=cache/'smoke-request.json'
            # Atomic immutable publication; no provider source checkout writes.
            temporary=cache/f'.smoke-request-{os.getpid()}.tmp';temporary.write_text(json.dumps(request));temporary.replace(request_path)
            args.product,args.request=product['product'],str(request_path)
        report = diagnose(compiler_checkout=os.environ['MNCS_COMPILER_CHECKOUT'],
            compiler_executable=os.environ['MNCS_COMPILER_PROBE'], vm_checkout=os.environ['MNCS_VM_CHECKOUT'],
            vm_executable=os.environ['MNCS_VM_BIN'], stage0=os.environ['MNCS_REFERENCE_BIN'],
            product_path=args.product, request_path=args.request, language_checkout=os.environ.get('MNCS_LANGUAGE_ROOT'))
        if args.smoke:
            report['smoke']={'cache_reused':product['cache_reused'],'product':product['product'],'expected_return':7}
            if args.reconcile_builds:
                report['build_repairs'] = repairs
            # The proof also checks its known answer; execution success alone is weaker.
            if report['status']=='pass':
                answer=report['compatibility']['returned']
                report['smoke']['returned']=answer
                if answer != [{'integer':{'value':7,'type':{'bits':64,'signed':True}}}]:
                    report['status']='fail';report['reason']='bounded smoke answer mismatch'
    except (OSError, KeyError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        report={'schema_version':'mncs.doctor.compiler-vm/1','status':'fail','reason':str(error)}
    if args.compact and isinstance(report.get('components'),dict):
        compiler=report['components'].get('compiler')
        if isinstance(compiler,dict) and isinstance(compiler.get('producer'),dict):
            receipt=compiler['producer'].get('receipt')
            if isinstance(receipt,dict) and isinstance(receipt.get('source_inputs'),dict):
                inputs=receipt.pop('source_inputs');receipt['source_input_count']=len(inputs)
                receipt['source_inputs_identity']=__import__('hashlib').sha256(json.dumps(inputs,sort_keys=True,separators=(',',':')).encode()).hexdigest()
        runtime=report['components'].get('runtime')
        embedded=((runtime or {}).get('contract') or {}).get('build_origin') if isinstance(runtime,dict) else None
        receipt=embedded.get('receipt') if isinstance(embedded,dict) else None
        if isinstance(receipt,dict) and isinstance(receipt.get('source_inputs'),dict):
            inputs=receipt.pop('source_inputs')
            receipt['source_input_count']=len(inputs)
            receipt['source_inputs_identity']=hashlib.sha256(
                json.dumps(inputs,sort_keys=True,separators=(',',':')).encode()
            ).hexdigest()
        compatibility = report.get('compatibility')
        if isinstance(compatibility, dict):
            compatibility['execution_provenance'] = compact_execution_provenance(
                compatibility.get('execution_provenance')
            )
        if isinstance(report.get('build_repairs'), list):
            report['build_repairs'] = [
                {
                    'provider': item.get('provider'),
                    'status': (item.get('result') or {}).get('status'),
                    'identity': (item.get('result') or {}).get('identity'),
                    'executable_sha256': (item.get('result') or {}).get('executable_sha256'),
                }
                for item in report['build_repairs'] if isinstance(item, dict)
            ]
    print(json.dumps(report,sort_keys=True)); return 0 if report['status'] in ('pass','unknown') else 2


if __name__ == '__main__':
    raise SystemExit(main())
