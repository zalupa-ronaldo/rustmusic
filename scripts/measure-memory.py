#!/usr/bin/env python3
"""Measure comparable application process trees on macOS/Linux without secrets."""
import argparse,csv,json,pathlib,subprocess,time
p=argparse.ArgumentParser();p.add_argument('--native-pid',type=int,required=True);p.add_argument('--browser-pid',type=int,required=True);p.add_argument('--seconds',type=int,default=1800);p.add_argument('--interval',type=int,default=5);p.add_argument('--output',default='.data/performance');args=p.parse_args()
folder=pathlib.Path(args.output);folder.mkdir(parents=True,exist_ok=True)
def snapshot():
 output=subprocess.check_output(['ps','-axo','pid=,ppid=,rss='],text=True)
 entries={}
 for line in output.splitlines():
  fields=line.split()
  if len(fields)==3:
   pid,ppid,rss=map(int,fields);entries[pid]=(ppid,rss)
 return entries
def tree_rss(entries,root):
 children={root};changed=True
 while changed:
  changed=False
  for pid,(parent,_) in entries.items():
   if parent in children and pid not in children:children.add(pid);changed=True
 return sum(entries[pid][1] for pid in children if pid in entries),len(children)
print('Use an isolated browser containing only YouTube Music.\n0–10 min: idle; 10–20 min: same track playing; 20–30 min: same searches, library and queue.\nReport total RSS, including helpers, not just the GUI process.',flush=True)
started=time.monotonic();rows=[]
while time.monotonic()-started<args.seconds:
 entries=snapshot()
 if args.native_pid not in entries or args.browser_pid not in entries:raise SystemExit('A root process exited; comparison invalid.')
 elapsed=round(time.monotonic()-started,1);native,nchildren=tree_rss(entries,args.native_pid);browser,bchildren=tree_rss(entries,args.browser_pid)
 rows.append({'seconds':elapsed,'native_rss_kib':native,'browser_rss_kib':browser,'native_processes':nchildren,'browser_processes':bchildren})
 print(f'{elapsed:.0f}s  native={native/1024:.1f} MiB  browser={browser/1024:.1f} MiB',flush=True)
 time.sleep(min(args.interval,max(0,args.seconds-(time.monotonic()-started))))
with (folder/'samples.csv').open('w') as f:
 writer=csv.DictWriter(f,fieldnames=rows[0].keys());writer.writeheader();writer.writerows(rows)
avg=lambda key:sum(r[key] for r in rows)/len(rows)
summary={'seconds':args.seconds,'samples':len(rows),'native_average_mib':avg('native_rss_kib')/1024,'native_peak_mib':max(r['native_rss_kib'] for r in rows)/1024,'browser_average_mib':avg('browser_rss_kib')/1024,'browser_peak_mib':max(r['browser_rss_kib'] for r in rows)/1024,'average_reduction_percent':100*(1-avg('native_rss_kib')/avg('browser_rss_kib'))}
(folder/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
