#!/usr/bin/env python3
"""Build a relocatable Apple Silicon .app/.dmg and collect source/license notices."""
import argparse, hashlib, json, os, pathlib, re, shutil, subprocess, tarfile
ROOT=pathlib.Path(__file__).resolve().parents[1]
DIST=ROOT/'dist'
APP=DIST/'RustMusic.app'
RES=APP/'Contents/Resources'
FRAME=APP/'Contents/Frameworks'
CACHE=ROOT/'target/packaging-cache'
def run(*args,**kwargs):
    return subprocess.check_output(list(map(str,args)),text=True,**kwargs).strip()
def download(url,path,sha=None):
    path.parent.mkdir(parents=True,exist_ok=True)
    if path.exists() and (not sha or hashlib.sha256(path.read_bytes()).hexdigest()==sha):return path
    candidates=[url]
    if sha and 'uchardet-' in path.name:candidates.append('https://distfiles.macports.org/uchardet/'+url.rsplit('/',1)[-1])
    partial=path.with_name(path.name+'.part')
    for candidate in candidates:
        command=['curl','--silent','--show-error','--fail','--location','--retry','2','--connect-timeout','15','--max-time','180','--output',str(partial),candidate]
        attempt=subprocess.run(command)
        if attempt.returncode!=0:partial.unlink(missing_ok=True);continue
        if sha and hashlib.sha256(partial.read_bytes()).hexdigest()!=sha:
            partial.unlink(missing_ok=True);raise RuntimeError('Checksum mismatch: '+path.name)
        partial.replace(path);return path
    raise RuntimeError('Source download failed: '+url)

def dependencies(path):
    output=run('otool','-L',path)
    return [line.strip().split(' (')[0] for line in output.splitlines()[1:]]
def rpaths(path):
    lines=run('otool','-l',path).splitlines();result=[]
    for i,line in enumerate(lines):
        if line.strip()=='cmd LC_RPATH':
            result.append(lines[i+2].strip().split(' (')[0].removeprefix('path '))
    return result

def resolved(dep,original):
    if dep.startswith('/'):return pathlib.Path(dep)
    if dep.startswith('@loader_path/'):
        return original.parent/dep.removeprefix('@loader_path/')
    if dep.startswith('@rpath/'):
        suffix=dep.removeprefix('@rpath/')
        for prefix in rpaths(original):
            base=prefix.replace('@loader_path',str(original.parent)).replace('@executable_path',str(original.parent))
            candidate=pathlib.Path(base)/suffix
            if candidate.exists():return candidate
    raise RuntimeError(f'Unresolved dependency {dep} in {original}')

formula_roots={}
mapping={}
rewrite=[]
def bundle(original,target):
    original=original.resolve()
    if original in mapping:return mapping[original]
    mapping[original]=target
    target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(original,target);target.chmod(0o755)
    if '/Cellar/' in str(original):
        rest=str(original).split('/Cellar/',1)[1].split('/')
        formula_roots[rest[0]]=pathlib.Path(str(original).split('/Cellar/',1)[0]+'/Cellar')/rest[0]/rest[1]
    for dep in dependencies(original):
        if dep.startswith('/System/') or dep.startswith('/usr/lib/'):continue
        source=resolved(dep,original).resolve()
        if source==original:continue
        basename=source.name
        destination=FRAME/basename
        if destination.exists() and source not in mapping:
            basename=hashlib.sha256(str(source).encode()).hexdigest()[:8]+'-'+basename
            destination=FRAME/basename
        destination=bundle(source,destination)
        relative=os.path.relpath(destination,target.parent)
        rewrite.append((target,dep,'@loader_path/'+relative))
    return target

def source_notices(manifest):
    sources=DIST/'third-party-sources';sources.mkdir(exist_ok=True)
    notices=RES/'licenses';notices.mkdir(parents=True,exist_ok=True)
    for formula,root in sorted(formula_roots.items()):
        print('Collecting sources:',formula,flush=True)
        info=json.loads(run('brew','info','--json=v2',formula))['formulae'][0]
        stable=info['urls']['stable'];receipt=json.loads((root/'INSTALL_RECEIPT.json').read_text().replace(str(pathlib.Path.home()), '$HOME'))
        source_version=receipt.get('source',{}).get('versions',{}).get('stable')
        if source_version and source_version!=info['versions']['stable']:
            recipe=(root/'.brew'/(formula+'.rb')).read_text()
            source_url=re.search(r'^\s*url "([^"]+)"',recipe,re.MULTILINE)
            source_sha=re.search(r'^\s*sha256 "([a-f0-9]{64})"',recipe,re.MULTILINE)
            if not source_url or not source_sha:raise RuntimeError(f'{formula}: matching source unavailable in installed recipe')
            stable={'url':source_url.group(1),'checksum':source_sha.group(1),'installed_version':source_version}
        entry={'name':formula,'installed':root.name,'license':info.get('license'),'source':stable,'receipt':receipt}
        manifest['homebrew'].append(entry)
        folder=sources/formula;folder.mkdir(exist_ok=True)
        (folder/'formula.json').write_text(json.dumps(info,indent=2)+'\n')
        (folder/'INSTALL_RECEIPT.json').write_text(json.dumps(receipt,indent=2)+'\n')
        brewfiles=root/'.brew'
        if brewfiles.exists():shutil.copytree(brewfiles,folder/'recipe',dirs_exist_ok=True)
        url=stable['url'];suffix=pathlib.Path(url.split('?')[0]).name or 'source.tar'
        archive=download(url,CACHE/(formula+'-'+suffix),stable.get('checksum'))
        shutil.copy2(archive,folder/suffix)
        # Preserve bottle-installed licensing files and source licensing documents.
        for file in root.rglob('*'):
            if file.is_file() and file.name.upper().startswith(('LICENSE','COPYING','COPYRIGHT','NOTICE')):
                destination=notices/formula/file.relative_to(root);destination.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(file,destination)
        if tarfile.is_tarfile(archive):
            with tarfile.open(archive) as tar:
                for member in tar.getmembers():
                    name=pathlib.PurePosixPath(member.name)
                    if member.isfile() and name.name.upper().startswith(('LICENSE','COPYING','COPYRIGHT','NOTICE')) and member.size<2*1024*1024:
                        if name.is_absolute() or '..' in name.parts:continue
                        destination=notices/formula/'upstream'/pathlib.Path(*name.parts)
                        destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(tar.extractfile(member).read())
        # Homebrew patches and exact formula revision are part of the corresponding source.
        for patch in info.get('patches',[]):
            if isinstance(patch,dict) and patch.get('url'):
                shutil.copy2(download(patch['url'],CACHE/(formula+'-patch-'+hashlib.sha256(patch['url'].encode()).hexdigest()[:12]),patch.get('sha256')),folder/('patch-'+hashlib.sha256(patch['url'].encode()).hexdigest()[:12]))
    yt_source=download('https://github.com/yt-dlp/yt-dlp/archive/refs/tags/2026.08.19.tar.gz',CACHE/'yt-dlp-2026.08.19.tar.gz')
    shutil.copy2(yt_source,sources/yt_source.name)
    download('https://raw.githubusercontent.com/yt-dlp/yt-dlp/2026.08.19/LICENSE',notices/'yt-dlp-LICENSE')
    deno_version=manifest['tools']['deno'].split()[1]
    download(f'https://raw.githubusercontent.com/denoland/deno/v{deno_version}/LICENSE.md',notices/'deno-LICENSE.md')
    # Rust dependencies are separate notices; do not redistribute their build artifacts.
    metadata=json.loads(run('cargo','metadata','--locked','--format-version','1',cwd=ROOT))
    for package in metadata['packages']:
        manifest['rust'].append({'name':package['name'],'version':package['version'],'license':package['license'],'repository':package['repository']})
        source=pathlib.Path(package['manifest_path']).parent
        for file in source.iterdir():
            if file.is_file() and file.name.upper().startswith(('LICENSE','COPYING','NOTICE')):
                folder=notices/'rust'/(package['name']+'-'+package['version']);folder.mkdir(parents=True,exist_ok=True);shutil.copy2(file,folder/file.name)
    shutil.copy2(ROOT/'docs/THIRD_PARTY.md',notices/'THIRD_PARTY.md')
    with tarfile.open(DIST/'third-party-sources.tar.gz','w:gz') as tar:tar.add(sources,arcname='third-party-sources')


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--skip-build',action='store_true');args=parser.parse_args()
    if run('uname','-m')!='arm64':raise RuntimeError('Build on an Apple Silicon Mac')
    if not args.skip_build:subprocess.run(['cargo','build','--locked','--release','-p','rustmusic'],cwd=ROOT,check=True)
    DIST.mkdir(exist_ok=True);CACHE.mkdir(parents=True,exist_ok=True)
    if APP.exists():shutil.rmtree(APP)
    (APP/'Contents/MacOS').mkdir(parents=True);RES.mkdir(parents=True);FRAME.mkdir(parents=True)
    shutil.copy2(ROOT/'packaging/Info.plist',APP/'Contents/Info.plist')
    bundle(ROOT/'target/release/rustmusic',APP/'Contents/MacOS/rustmusic')
    for tool in ['mpv','deno']:
        path=shutil.which(tool)
        if not path:raise RuntimeError('Install build dependency: brew install '+tool)
        bundle(pathlib.Path(path),RES/'bin'/tool)
    checksums=download('https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/SHA2-256SUMS',CACHE/'yt-dlp-2026.08.19-SHA2-256SUMS')
    sha=next(line.split()[0] for line in checksums.read_text().splitlines() if line.split()[-1]=='yt-dlp_macos')
    yt=download('https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp_macos',CACHE/'yt-dlp_macos-2026.08.19',sha)
    shutil.copy2(yt,RES/'bin/yt-dlp');(RES/'bin/yt-dlp').chmod(0o755)
    for target,old,new in rewrite:subprocess.run(['install_name_tool','-change',old,new,str(target)],check=True,stdout=subprocess.DEVNULL)
    for target in mapping.values():
        if target.suffix=='.dylib':subprocess.run(['install_name_tool','-id','@rpath/'+target.name,str(target)],check=True,stdout=subprocess.DEVNULL)
    for target in mapping.values():
        subprocess.run(['codesign','--force','--sign','-',str(target)],check=True,stdout=subprocess.DEVNULL)
    minimum=(13,0)
    for target in mapping.values():
        lines=run('otool','-l',target).splitlines()
        for line in lines:
            if line.strip().startswith('minos '):
                version=tuple(map(int,line.strip().split()[1].split('.')))
                minimum=max(minimum,version)
    subprocess.run(['plutil','-replace','LSMinimumSystemVersion','-string','.'.join(map(str,minimum)),str(APP/'Contents/Info.plist')],check=True)
    manifest={'architecture':'arm64','tools':{'yt-dlp':'2026.08.19','mpv':run(RES/'bin/mpv','--version').splitlines()[0],'deno':run(RES/'bin/deno','--version').splitlines()[0]},'homebrew':[],'rust':[]}
    source_notices(manifest)
    (DIST/'dependencies.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n');shutil.copy2(DIST/'dependencies.json',RES/'dependencies.json')
    subprocess.run(['python3',str(ROOT/'scripts/make-icon.py'),str(RES)],check=True)
    for target in list(mapping.values())+[RES/'bin/yt-dlp']:
        subprocess.run(['codesign','--force','--sign','-',str(target)],check=True,stdout=subprocess.DEVNULL)
    subprocess.run(['codesign','--force','--deep','--sign','-',str(APP)],check=True)
    subprocess.run(['codesign','--verify','--deep','--strict',str(APP)],check=True)
    for target in mapping.values():
        for dep in dependencies(target):
            if '/opt/homebrew/' in dep or '/usr/local/' in dep:raise RuntimeError('Non-relocatable dependency: '+dep)
    staging=DIST/'dmg-content';staging.mkdir(exist_ok=True)
    if (staging/'RustMusic.app').exists():shutil.rmtree(staging/'RustMusic.app')
    shutil.copytree(APP,staging/'RustMusic.app',symlinks=True)
    if not (staging/'Applications').exists():os.symlink('/Applications',staging/'Applications')
    dmg=DIST/'RustMusic-0.1.0-macOS-arm64.dmg'
    subprocess.run(['hdiutil','create','-volname','RustMusic','-srcfolder',str(staging),'-ov','-format','UDZO',str(dmg)],check=True)
    for artifact in [dmg,DIST/'third-party-sources.tar.gz']:
        (DIST/(artifact.name+'.sha256')).write_text(hashlib.sha256(artifact.read_bytes()).hexdigest()+'  '+artifact.name+'\n')
    print('Built:',dmg)
if __name__=='__main__':main()
