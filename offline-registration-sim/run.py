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
 'backend/src/connectivity/modems/ims/cellular_ims/ipsec.rs',
 'backend/src/connectivity/modems/ims/cellular_ims/channel.rs',
 'backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs',
 'offline-registration-sim/simulator.rs','offline-registration-sim/cellular_adapter.rs',
 'offline-registration-sim/wifi_adapter.rs',
 'offline-registration-sim/run.py',
]
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def fingerprints():return {name:sha(ROOT/name) for name in SOURCES}

def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--report',type=Path,default=ROOT/'offline-registration-sim/results/verified.json')
 p.add_argument('--cargo',default=shutil.which('cargo') or str(Path.home()/'.cargo/bin/cargo'))
 p.add_argument('--target-dir',type=Path)
 args=p.parse_args();report=args.report.resolve()
 if report.exists():p.error('report exists; select a new path to preserve previous evidence')
 report.parent.mkdir(parents=True,exist_ok=True)
 raw=report.with_suffix('.raw.json');log=report.with_suffix('.log')
 if raw.exists() or log.exists():p.error('evidence sidecar already exists')
 before=fingerprints()
 env={**os.environ,'SIMADMIN_DERIVATION_REPORT':str(raw)}
 if args.target_dir:env['CARGO_TARGET_DIR']=str(args.target_dir.resolve())
 env['PATH']=str(Path(args.cargo).parent)+os.pathsep+env.get('PATH','')
 command=[args.cargo,'test','--manifest-path',str(ROOT/'backend/Cargo.toml'),'--locked','--offline',
          'offline_derivation_registration_matrix','--','--nocapture','--test-threads=1']
 with log.open('w',encoding='utf-8') as out:
  result=subprocess.run(command,cwd=ROOT,env=env,stdout=out,stderr=subprocess.STDOUT,timeout=1200)
 if result.returncode or not raw.is_file():raise RuntimeError('simulation failed; inspect '+str(log))
 if not re.search(r'test result: ok\. 1 passed; 0 failed;',log.read_text()):raise RuntimeError('the intended matrix did not execute')
 if before!=fingerprints():raise RuntimeError('source changed during simulation')
 data=json.loads(raw.read_text())
 if data.get('passed') is not True or data.get('live_network_verified') is not False or data.get('hardware_used') is not False:
  raise RuntimeError('invalid simulation evidence flags')
 cases=data['scenarios']
 if len(cases)!=24 or len({c['id'] for c in cases})!=24 or not all(c['passed'] is True and c['expected_success']==c['observed_success'] for c in cases):
  raise RuntimeError('matrix incomplete or expectations did not hold')
 data.update(report_format=1,source_files_sha256=before,
   source_tree_sha256=hashlib.sha256(json.dumps(before,sort_keys=True,separators=(',',':')).encode()).hexdigest(),
   log_sha256=sha(log),test_count=1,
   interpretation='Fixture evidence only. Pruning is limited to declared standard requirements covered by this model; unknown or stricter carrier policies must remain.')
 report.write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
 print(json.dumps({'report':str(report),'scenarios':len(cases),'registered_in_fixture':sum(c['observed_success'] for c in cases),
   'expected_rejections':sum(not c['observed_success'] for c in cases),'passed':True,'live_network_verified':False},indent=2))
if __name__=='__main__':
 try:main()
 except Exception as error:print('simulation runner failed: '+str(error),file=sys.stderr);raise SystemExit(1)
