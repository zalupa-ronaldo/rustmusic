import './style.css';
type Kind = 'track'|'album'|'artist'|'playlist';
type Item = {id:string;title:string;subtitle:string;kind:Kind;thumbnail?:string;duration?:string;set_video_id?:string};
type Page = {items:Item[];continuation?:string};
type Route = {type:'browse'|'search'|'radio';value:string;title:string};
type Queue = {items:Item[];index:number|null;repeat:'off'|'all'|'one'};
const icons:Record<string,string>={music:'♪',home:'⌂',library:'▤',heart:'♡',search:'⌕',settings:'⚙',play:'▶',pause:'Ⅱ',next:'▸▸',previous:'◂◂',shuffle:'⤨',repeat:'↻',plus:'＋',close:'×',more:'•••'};
const app=document.querySelector<HTMLDivElement>('#app')!;
app.innerHTML=`<aside><a class="brand" href="/">${icons.music}<span>RustMusic<small>YOUR MUSIC, LESS MEMORY</small></span></a><nav><button data-nav="FEmusic_home">${icons.home} Для вас</button><button data-nav="FEmusic_liked_playlists">${icons.library} Библиотека</button><button data-nav="VLLM">${icons.heart} Любимые треки</button><button data-nav="FEmusic_liked_albums">▣ Мои альбомы</button><button data-nav="FEmusic_liked_artists">♬ Исполнители</button></nav><div class="queue-label">ОЧЕРЕДЬ <span id="queue-count">0</span></div><div id="queue"></div><div class="aside-bottom"><span id="account-name">Аккаунт не подключён</span><button id="settings-button">${icons.settings} Настройки</button></div></aside><main><header><form id="search"><span>${icons.search}</span><input id="query" placeholder="Трек, альбом или исполнитель" autocomplete="off"><button>Найти</button></form><span class="badge">RUST · WEB CLIENT</span></header><section class="heading"><div><p>YOUTUBE MUSIC</p><h1 id="page-title">Для вас</h1><span id="page-subtitle">Всё, что хочется слушать</span></div><div id="playlist-tools"></div></section><div id="notice" role="status"></div><section id="items"></section><button id="more" hidden>Загрузить ещё</button></main><footer><div class="now-playing"><div id="now-cover">♪</div><div><strong id="now-title">Выберите музыку</strong><span id="now-artist">Ваша библиотека. Ваш ритм.</span></div></div><div class="transport"><div class="transport-buttons"><button id="shuffle" title="Перемешать">${icons.shuffle}</button><button id="previous" title="Предыдущий">${icons.previous}</button><button id="play" class="play" title="Воспроизведение / пауза">${icons.play}</button><button id="next" title="Следующий">${icons.next}</button><button id="repeat" title="Повтор выключен">${icons.repeat}</button></div><div class="timeline"><time id="position">0:00</time><input id="seek" type="range" min="0" max="1" value="0" step="0.1" aria-label="Позиция трека"><time id="duration">0:00</time></div></div><div class="volume"><span>♫</span><input id="volume" type="range" min="0" max="1" step="0.01" value="0.8" aria-label="Громкость"></div></footer><dialog id="settings"><button class="dialog-close" data-close="settings">×</button><h2>Подключить YouTube Music</h2><p>Войдите в Google в своём браузере и импортируйте сессию. Контейнер не читает браузерные профили компьютера.</p><a href="https://music.youtube.com" target="_blank" rel="noopener noreferrer">Открыть YouTube Music ↗</a><form id="import"><label>Сессия: JSON headers, Cookie или Netscape cookies.txt<textarea id="session-input" rows="5" spellcheck="false" autocomplete="off" required></textarea></label><label class="file">Выбрать файл<input id="session-file" type="file" accept=".json,.txt"></label><label>Номер аккаунта Google<input id="auth-user" type="number" min="0" max="255" value="0"></label><p class="muted">Сессия хранится зашифрованной в volume. Не передавайте её другим людям.</p><button class="primary">Проверить и сохранить</button></form><div class="settings-actions"><button id="disconnect">Удалить сессию Google</button><button id="web-logout">Выйти из сервера</button></div></dialog><dialog id="login"><h2>Ваш сервер RustMusic</h2><p>Введите ключ из <code>.secrets/server-key</code>, созданный скриптом настройки Docker.</p><form id="login-form"><input id="password" type="password" autocomplete="current-password" placeholder="Ключ сервера" required><button class="primary">Войти</button><p id="login-error" role="alert"></p></form></dialog><audio id="audio" preload="none"></audio>`;
const $=<T extends HTMLElement=HTMLElement>(id:string)=>document.getElementById(id) as T;
const audio=$<HTMLAudioElement>('audio');audio.volume=.8;
let route:Route={type:'browse',value:'FEmusic_home',title:'Для вас'};
let items:Item[]=[];let continuation:string|undefined;let authenticated=false;let fetchController:AbortController|undefined;let events:EventSource|undefined;let playGeneration=0;
let queue:Queue={items:[],index:null,repeat:'off'};
const escape=(s:string)=>s.replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]!));
function notice(message:string,error=false){$('notice').textContent=message;$('notice').classList.toggle('error',error);}
async function api<T>(path:string,options:RequestInit={}):Promise<T>{
 const response=await fetch(`/api/v1${path}`,{...options,headers:{'content-type':'application/json','x-rustmusic':'1',...options.headers}});
 if(response.status===401){$<HTMLDialogElement>('login').showModal();throw new Error('Требуется вход в сервер');}
 if(!response.ok){let message='Запрос не выполнен';try{message=(await response.json()).error??message;}catch{}throw new Error(message);}
 return response.json() as Promise<T>;
}
const run=(fn:()=>Promise<unknown>)=>{void fn().catch(e=>notice(e.message,true));};
function cover(item:Item){return item.thumbnail?`<img src="${escape(item.thumbnail)}" loading="lazy" alt="" referrerpolicy="no-referrer">`:`<span class="cover-placeholder">♪</span>`;}
function render(){
 $('page-title').textContent=route.title;
 $('page-subtitle').textContent=items.length?`${items.length} элементов · YouTube Music`:'Всё, что хочется слушать';
 document.querySelectorAll<HTMLButtonElement>('[data-nav]').forEach(b=>b.classList.toggle('active',route.value===b.dataset.nav));
 const collections=items.map((item,index)=>({item,index})).filter(({item})=>item.kind!=='track');
 const tracks=items.map((item,index)=>({item,index})).filter(({item})=>item.kind==='track');
 $('items').innerHTML=`${collections.length?`<div class="cards">${collections.map(({item,index})=>`<button class="card" data-open="${index}"><div class="card-cover">${cover(item)}<span class="card-play">▶</span></div><strong>${escape(item.title)}</strong><small>${escape(item.subtitle||({album:'Альбом',artist:'Исполнитель',playlist:'Плейлист'}[item.kind as 'album'|'artist'|'playlist']))}</small></button>`).join('')}</div>`:''}${tracks.length?`<div class="track-list"><div class="track-header"><span>ТРЕК</span><span>ДЛИТЕЛЬНОСТЬ</span></div>${tracks.map(({item,index},n)=>`<div class="track"><span class="track-number">${n+1}</span><button class="track-open" data-open="${index}">${cover(item)}<span><strong>${escape(item.title)}</strong><small>${escape(item.subtitle)}</small></span></button><span class="track-duration">${escape(item.duration??'—')}</span><details><summary aria-label="Действия с треком">•••</summary><div class="track-menu"><button data-enqueue="${index}">В очередь</button><button data-radio="${index}">Радио трека</button>${authenticated?`<button data-like="${index}">Поставить лайк</button><button data-unlike="${index}">Снять лайк</button><button data-add="${index}">Добавить в плейлист</button>${item.set_video_id&&route.type==='browse'?`<button data-remove="${index}">Убрать из плейлиста</button>${n>0?`<button data-move="${index}">Поднять выше</button>`:''}`:''}`:''}</div></details></div>`).join('')}</div>`:''}${!items.length?'<div class="empty"><span>♫</span><h2>Музыка начинается здесь</h2><p>Попробуйте поиск или подключите свою библиотеку.</p></div>':''}`;
 $<HTMLButtonElement>('more').hidden=!continuation;
 const editable=route.type==='browse'&&(route.value.startsWith('PL')||route.value.startsWith('VLPL'));
 $('playlist-tools').innerHTML=authenticated?`<button id="create-playlist">＋ Плейлист</button>${editable?'<button id="rename-playlist">Переименовать</button><button id="delete-playlist">Удалить</button>':''}`:'';
}
async function navigate(next:Route,more=false){
 fetchController?.abort();const controller=new AbortController();fetchController=controller;
 route=next;if(!more){items=[];continuation=undefined;render();}notice('Загрузка…');
 $<HTMLButtonElement>('more').disabled=true;
 try{
  const params=new URLSearchParams();if(next.type==='search')params.set('q',next.value);if(more&&continuation)params.set('continuation',continuation);
  const path=next.type==='search'?'/search':`/${next.type}/${encodeURIComponent(next.value)}`;
  const page=await api<Page>(`${path}?${params}`,{signal:controller.signal});
  if(controller.signal.aborted)return;
  items=more?[...items,...page.items].slice(-500):page.items;continuation=page.continuation;render();notice(items.length?'':'В этом разделе пока нет элементов');
 }catch(e){if(!controller.signal.aborted)notice((e as Error).message,true);}finally{if(!controller.signal.aborted)$<HTMLButtonElement>('more').disabled=false;}
}
function renderQueue(){
 $('queue-count').textContent=String(queue.items.length);
 $('queue').innerHTML=queue.items.map((item,index)=>`<button class="queue-item ${queue.index===index?'active':''}" data-queue="${index}"><span>${queue.index===index?'▶':'♪'}</span><span><strong>${escape(item.title)}</strong><small>${escape(item.subtitle)}</small></span></button>`).join('');
 $('repeat').classList.toggle('active',queue.repeat!=='off');$('repeat').textContent=queue.repeat==='one'?'↻₁':'↻';$('repeat').title=`Повтор: ${{off:'выключен',all:'все',one:'один'}[queue.repeat]}`;
}
async function queueCommand(command:Record<string,unknown>){queue=await api<Queue>('/queue',{method:'POST',body:JSON.stringify(command)});renderQueue();}
async function playCurrent(){
 const generation=++playGeneration;audio.pause();audio.removeAttribute('src');audio.load();
 const item=queue.index!==null?queue.items[queue.index]:undefined;if(!item){$('now-title').textContent='Очередь закончилась';return;}
 $('now-title').textContent=item.title;$('now-artist').textContent=item.subtitle;$('now-cover').innerHTML=cover(item);renderQueue();notice('Получение потока…');
 const resolved=await api<{url:string}>(`/resolve/${encodeURIComponent(item.id)}`,{method:'POST'});if(generation!==playGeneration)return;
 audio.src=resolved.url;await audio.play();notice('');
 if('mediaSession' in navigator){navigator.mediaSession.metadata=new MediaMetadata({title:item.title,artist:item.subtitle,artwork:item.thumbnail?[{src:item.thumbnail}]:[]});}
}
async function openItem(index:number){
 const item=items[index];if(!item)return;
 if(item.kind!=='track'){await navigate({type:'browse',value:item.id,title:item.title});return;}
 const tracks=items.filter(i=>i.kind==='track');await queueCommand({action:'replace',items:tracks,index:tracks.findIndex(i=>i===item)});await playCurrent();
}
async function advance(ended=false){await queueCommand({action:'next',ended});await playCurrent();}
async function previous(){if(audio.currentTime>3){audio.currentTime=0;return;}await queueCommand({action:'previous'});await playCurrent();}
async function mutate(action:Record<string,unknown>){notice('Сохранение…');const value=await api<{playlist_id?:string}>('/mutate',{method:'POST',body:JSON.stringify(action)});if(value.playlist_id)await navigate({type:'browse',value:value.playlist_id,title:String(action.title)});else await navigate(route);notice('Изменение сохранено');}
$('items').addEventListener('click',event=>{
 const button=(event.target as HTMLElement).closest<HTMLButtonElement>('button');if(!button)return;
 const data=button.dataset;const index=Number(Object.values(data)[0]);const item=items[index];
 if(data.open!==undefined)run(()=>openItem(index));
 if(data.enqueue!==undefined&&item)run(()=>queueCommand({action:'append',item}));
 if(data.radio!==undefined&&item)run(()=>navigate({type:'radio',value:item.id,title:`Радио · ${item.title}`}));
 if(data.like!==undefined||data.unlike!==undefined)run(()=>mutate({action:'like',video_id:item.id,liked:data.like!==undefined}));
 if(data.add!==undefined){const id=prompt('ID плейлиста (например PL…):');if(id)run(()=>mutate({action:'add',playlist_id:id.trim().replace(/^VL/,''),video_id:item.id}));}
 if(data.remove!==undefined&&confirm(`Убрать «${item.title}» из плейлиста?`))run(()=>mutate({action:'remove',playlist_id:route.value.replace(/^VL/,''),video_id:item.id,set_video_id:item.set_video_id}));
 if(data.move!==undefined){const before=items.slice(0,index).reverse().find(i=>i.kind==='track'&&i.set_video_id);if(before)run(()=>mutate({action:'move',playlist_id:route.value.replace(/^VL/,''),set_video_id:item.set_video_id,before_set_video_id:before.set_video_id}));}
});
$('playlist-tools').addEventListener('click',event=>{
 const id=(event.target as HTMLElement).closest('button')?.id;
 if(id==='create-playlist'){const title=prompt('Название нового приватного плейлиста:');if(title)run(()=>mutate({action:'create',title}));}
 if(id==='rename-playlist'){const title=prompt('Новое название плейлиста:');if(title)run(()=>mutate({action:'rename',playlist_id:route.value.replace(/^VL/,''),title}));}
 if(id==='delete-playlist'&&confirm('Удалить этот плейлист из вашего аккаунта?'))run(async()=>{await api('/mutate',{method:'POST',body:JSON.stringify({action:'delete',playlist_id:route.value.replace(/^VL/,'')})});await navigate({type:'browse',value:'FEmusic_liked_playlists',title:'Библиотека'});});
});
document.querySelectorAll<HTMLButtonElement>('[data-nav]').forEach(button=>button.addEventListener('click',()=>run(()=>navigate({type:'browse',value:button.dataset.nav!,title:button.textContent!.trim().slice(2)}))));
$('search').addEventListener('submit',e=>{e.preventDefault();const value=$<HTMLInputElement>('query').value.trim();if(value)run(()=>navigate({type:'search',value,title:`Поиск · ${value}`}));});
$('more').addEventListener('click',()=>run(()=>navigate(route,true)));
$('queue').addEventListener('click',event=>{const button=(event.target as HTMLElement).closest<HTMLButtonElement>('[data-queue]');if(button)run(async()=>{await queueCommand({action:'select',index:Number(button.dataset.queue)});await playCurrent();});});
$('play').addEventListener('click',()=>{if(audio.src){if(audio.paused)run(()=>audio.play());else audio.pause();}});
$('next').addEventListener('click',()=>run(()=>advance()));$('previous').addEventListener('click',()=>run(previous));
$('shuffle').addEventListener('click',()=>run(()=>queueCommand({action:'shuffle'})));$('repeat').addEventListener('click',()=>run(()=>queueCommand({action:'repeat'})));
$('volume').addEventListener('input',()=>{audio.volume=Number($<HTMLInputElement>('volume').value);});
$('seek').addEventListener('change',()=>{if(Number.isFinite(audio.duration))audio.currentTime=Number($<HTMLInputElement>('seek').value);});
const time=(seconds:number)=>`${Math.floor((seconds||0)/60)}:${String(Math.floor((seconds||0)%60)).padStart(2,'0')}`;
audio.addEventListener('timeupdate',()=>{$('position').textContent=time(audio.currentTime);$<HTMLInputElement>('seek').value=String(audio.currentTime);});
audio.addEventListener('loadedmetadata',()=>{const duration=Number.isFinite(audio.duration)?audio.duration:0;$('duration').textContent=time(duration);$<HTMLInputElement>('seek').max=String(duration||1);});
audio.addEventListener('play',()=>{$('play').textContent=icons.pause;});audio.addEventListener('pause',()=>{$('play').textContent=icons.play;});audio.addEventListener('ended',()=>run(()=>advance(true)));
audio.addEventListener('error',()=>{if(audio.getAttribute('src'))notice('Аудиопоток недоступен. Повторите выбор трека или подключите сессию заново.',true);});
if('mediaSession' in navigator){navigator.mediaSession.setActionHandler('play',()=>run(()=>audio.play()));navigator.mediaSession.setActionHandler('pause',()=>audio.pause());navigator.mediaSession.setActionHandler('nexttrack',()=>run(()=>advance()));navigator.mediaSession.setActionHandler('previoustrack',()=>run(previous));navigator.mediaSession.setActionHandler('seekto',details=>{if(details.seekTime!==undefined)audio.currentTime=details.seekTime;});}
$('settings-button').addEventListener('click',()=>$<HTMLDialogElement>('settings').showModal());
document.querySelectorAll<HTMLButtonElement>('[data-close]').forEach(button=>button.addEventListener('click',()=>$<HTMLDialogElement>(button.dataset.close!).close()));
$('session-file').addEventListener('change',()=>{run(async()=>{const file=$<HTMLInputElement>('session-file').files?.[0];if(file){if(file.size>256*1024)throw new Error('Файл слишком большой');$<HTMLTextAreaElement>('session-input').value=await file.text();}});});
$('import').addEventListener('submit',event=>{event.preventDefault();run(async()=>{notice('Проверка аккаунта…');const value=await api<{account:{name:string}}>('/session',{method:'POST',body:JSON.stringify({input:$<HTMLTextAreaElement>('session-input').value,auth_user:$<HTMLInputElement>('auth-user').value})});$<HTMLTextAreaElement>('session-input').value='';$<HTMLInputElement>('session-file').value='';authenticated=true;$('account-name').textContent=value.account.name;$<HTMLDialogElement>('settings').close();await navigate({type:'browse',value:'FEmusic_home',title:'Для вас'});});});
$('disconnect').addEventListener('click',()=>run(async()=>{await api('/session',{method:'DELETE'});audio.pause();audio.removeAttribute('src');audio.load();authenticated=false;$('account-name').textContent='Аккаунт не подключён';$<HTMLDialogElement>('settings').close();await navigate({type:'browse',value:'FEmusic_home',title:'Для вас'});}));
$('web-logout').addEventListener('click',()=>run(async()=>{await api('/logout',{method:'POST'});audio.pause();events?.close();$<HTMLDialogElement>('settings').close();$<HTMLDialogElement>('login').showModal();}));
$('login-form').addEventListener('submit',event=>{event.preventDefault();void(async()=>{try{await api('/login',{method:'POST',body:JSON.stringify({password:$<HTMLInputElement>('password').value})});$<HTMLInputElement>('password').value='';$<HTMLDialogElement>('login').close();await start();}catch(e){$('login-error').textContent=(e as Error).message;}})();});
$<HTMLDialogElement>('login').addEventListener('cancel',event=>event.preventDefault());
async function start(){const status=await api<{account:{name:string}|null}>('/status');authenticated=!!status.account;$('account-name').textContent=status.account?.name??'Аккаунт не подключён';queue=await api<Queue>('/queue');renderQueue();events?.close();events=new EventSource('/api/v1/events');events.addEventListener('progress',event=>notice((event as MessageEvent).data));await navigate(route);}
render();run(start);
