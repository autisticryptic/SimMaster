#!/usr/bin/env python3
"""Run only the hardware-free real-code REGISTER matrix and bind its evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[1]
SOURCES=[
 'backend/src/connectivity/core/register.rs','backend/src/connectivity/core/access.rs',
 'backend/src/connectivity/core/digest_aka.rs','backend/src/connectivity/core/register_message.rs',
 'backend/src/connectivity/core/sip_frame.rs',
 'backend/src/connectivity/modems/ims/vowifi/profiles.rs',
 'backend/src/connectivity/modems/ims/vowifi/live.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/live.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/sip.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/security_agreement.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/security_hint.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/register_failure_diagnostics.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/ipsec.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/channel.rs',
 'backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs',
 'offline-registration-sim/simulator.rs','offline-registration-sim/cellular_adapter.rs',
 'offline-registration-sim/wifi_adapter.rs','offline-registration-sim/history.rs',
 'offline-registration-sim/security_hint.rs',
 'offline-registration-sim/run.py',
]
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def fingerprints():return {name:sha(ROOT/name) for name in SOURCES}

def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--report',type=Path,default=ROOT/'offline-registration-sim/results/verified.json')
 p.add_argument('--cargo',default=shutil.which('cargo') or str(Path.home()/'.cargo/bin/cargo'))
 p.add_argument('--target-dir',type=Path)
 matrix=p.add_mutually_exclusive_group()
 matrix.add_argument('--history',action='store_true',help='run history-inspired protocol regressions instead of the pruning evidence matrix')
 matrix.add_argument('--security-hint',action='store_true',help='run CMCC-shaped synthetic 421/494 hint regressions (not pruning evidence)')
 args=p.parse_args()
 # This maintenance workflow must never silently fall back to a local build.
 # Check before creating evidence files or invoking any compiler subprocess.
 if os.environ.get('GITHUB_ACTIONS') != 'true':
  p.error('compilation and registration simulation are restricted to GitHub Actions')
 report=args.report.resolve()
 if report.exists():p.error('report exists; select a new path to preserve previous evidence')
 report.parent.mkdir(parents=True,exist_ok=True)
 raw=report.with_suffix('.raw.json');log=report.with_suffix('.log')
 if raw.exists() or log.exists():p.error('evidence sidecar already exists')
 before=fingerprints()
 env={**os.environ,('SIMADMIN_SECURITY_HINT_REPORT' if args.security_hint else 'SIMADMIN_HISTORY_REPORT' if args.history else 'SIMADMIN_DERIVATION_REPORT'):str(raw)}
 if args.target_dir:env['CARGO_TARGET_DIR']=str(args.target_dir.resolve())
 env['PATH']=str(Path(args.cargo).parent)+os.pathsep+env.get('PATH','')
 command=[args.cargo,'test','--manifest-path',str(ROOT/'backend/Cargo.toml'),'--locked','--offline',
          ('offline_security_hint_registration_matrix' if args.security_hint else 'offline_historical_registration_matrix' if args.history else 'offline_derivation_registration_matrix'),'--','--nocapture','--test-threads=1']
 with log.open('w',encoding='utf-8') as out:
  result=subprocess.run(command,cwd=ROOT,env=env,stdout=out,stderr=subprocess.STDOUT,timeout=1200)
 if result.returncode or not raw.is_file():raise RuntimeError('simulation failed; inspect '+str(log))
 if not re.search(r'test result: ok\. 1 passed; 0 failed;',log.read_text()):raise RuntimeError('the intended matrix did not execute')
 if before!=fingerprints():raise RuntimeError('source changed during simulation')
 data=json.loads(raw.read_text())
 if data.get('passed') is not True or data.get('live_network_verified') is not False or data.get('hardware_used') is not False:
  raise RuntimeError('invalid simulation evidence flags')
 cases=data['scenarios']
 expected_count=12 if args.security_hint else 18 if args.history else 24
 if len(cases)!=expected_count or len({c['id'] for c in cases})!=expected_count or not all(c['passed'] is True and c['expected_success']==c['observed_success'] for c in cases):
  raise RuntimeError('matrix incomplete or expectations did not hold')
 data.update(report_format=1,source_files_sha256=before,
   execution_environment='github_actions' if os.environ.get('GITHUB_ACTIONS')=='true' else 'other',
   github_actions={key:os.environ.get(name) for key,name in (
     ('repository','GITHUB_REPOSITORY'),('commit','GITHUB_SHA'),('run_id','GITHUB_RUN_ID'),
     ('run_attempt','GITHUB_RUN_ATTEMPT'),('workflow','GITHUB_WORKFLOW'))}
     if os.environ.get('GITHUB_ACTIONS')=='true' else None,
   source_tree_sha256=hashlib.sha256(json.dumps(before,sort_keys=True,separators=(',',':')).encode()).hexdigest(),
   log_sha256=sha(log),test_count=1,
   interpretation=('Synthetic 421/494 security-hint regression only; real server offer values were absent from the log, and this does not authorize pruning or certify CMCC.' if args.security_hint else 'History-inspired protocol regression only; does not authorize additional catalog pruning or certify historical carriers.' if args.history else 'Fixture evidence only. Pruning is limited to declared standard requirements covered by this model; unknown or stricter carrier policies must remain.'))
 report.write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
 print(json.dumps({'report':str(report),'scenarios':len(cases),'registered_in_fixture':sum(c['observed_success'] for c in cases),
   'expected_rejections':sum(not c['observed_success'] for c in cases),'passed':True,'live_network_verified':False},indent=2))
if __name__=='__main__':
 try:main()
 except Exception as error:print('simulation runner failed: '+str(error),file=sys.stderr);raise SystemExit(1)
