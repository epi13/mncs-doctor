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
from pathlib import Path


def diagnose(*, compiler_checkout, compiler_executable, vm_checkout, vm_executable, stage0, product_path=None, request_path=None):
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
    reference = Path(stage0).resolve()
    reference_probe = subprocess.run([str(reference), 'language-inventory'], capture_output=True, timeout=10, check=False)
    report['components']['reference'] = {'executable':str(reference), 'executable_sha256':sha(reference),
        'callable':reference_probe.returncode==0, 'build_origin':'unknown; exact selected executable observed'}
    compatible = (producer.get('artifact_schema') == runtime['contract']['artifact_schema']
        and producer.get('vm_contract') == runtime['contract']['vm_contract']
        and producer.get('ssa_schema') in runtime['contract']['ssa_schemas'])
    report['compatibility'] = {'declarations_agree':compatible, 'admission':'not_probed', 'execution':'not_probed'}
    if producer['state'] != 'ready' or not compatible or reference_probe.returncode:
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


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--product')
    parser.add_argument('--request')
    parser.add_argument('--smoke',action='store_true',help='provider-owned bounded direct compiler/VM proof')
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
            product_path=args.product, request_path=args.request)
        if args.smoke:
            report['smoke']={'cache_reused':product['cache_reused'],'product':product['product'],'expected_return':7}
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
    print(json.dumps(report,sort_keys=True)); return 0 if report['status'] in ('pass','unknown') else 2


if __name__ == '__main__':
    raise SystemExit(main())
