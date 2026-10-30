(()=>{'use strict';
const results=document.getElementById('results'), index=document.getElementById('index-items');
const query=document.getElementById('query'), status=document.getElementById('search-status');
const send=m=>window.ipc?.postMessage(JSON.stringify(m));
const sections=new Map(), frames=new Map(), loaded=new Map();let currentQuery=null, active=null, scheduled=false, trimTimer;
const reduced=()=>window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
const hex=v=>typeof v==='string'&&/^[0-9a-f]{6}$/i.test(v);
let pinned=false;
window.setPinned=value=>{pinned=!!value;document.getElementById('pin').setAttribute('aria-pressed',String(pinned));};
function appearance(state){
 const palettes={light:['fafaf7','292d2c','69716e','dce0dc'],dark:['202426','e7e7df','a2ada8','394144'],sepia:['f5efdf','463e31','847561','dcd0b7']};
 const p=palettes[state.theme]||palettes.light, style=document.documentElement.style;
 const paper=hex(state.paper)?state.paper:p[0],ink=hex(state.ink)?state.ink:p[1];
 style.setProperty('--paper','#'+paper);style.setProperty('--ink','#'+ink);
 // Custom chrome keeps all labels readable; authored articles retain their colors.
 style.setProperty('--muted',hex(state.ink)?'#'+ink:p[2].replace(/^/,'#'));
 style.setProperty('--line',hex(state.ink)?'#'+ink+'30':'#'+p[3]);
 const accent=hex(state.accent)?state.accent:'2563eb';style.setProperty('--accent','#'+accent);style.setProperty('--tint','#'+accent+'20');
 document.documentElement.style.colorScheme=state.theme==='dark'?'dark':'light';
}
function setActive(id,focus=false){
 if(!sections.has(id))return;active=id;
 for(const [key,value] of sections){const on=key===id;value.button.tabIndex=on?0:-1;
  if(on)value.button.setAttribute('aria-current','location');else value.button.removeAttribute('aria-current');}
 const button=sections.get(id).button;
 const rect=button.getBoundingClientRect(), bounds=index.getBoundingClientRect();
 if(rect.top<bounds.top)index.scrollTop-=bounds.top-rect.top;else if(rect.bottom>bounds.bottom)index.scrollTop+=rect.bottom-bounds.bottom;
 if(focus)button.focus({preventScroll:true});
}
function jump(id,focus=false){
 const value=sections.get(id);if(!value)return;setActive(id,focus);
 const top=value.section.getBoundingClientRect().top-results.getBoundingClientRect().top+results.scrollTop;
 results.scrollTo({top,behavior:reduced()?'instant':'smooth'});
 send({action:'dictionary',dictionary_id:id});
}
function scrollspy(){scheduled=false;let last=null;
 const top=results.getBoundingClientRect().top;
 for(const [id,value] of sections){if(value.section.getBoundingClientRect().top<=top+24)last=id;else break;}
 if(!last)last=sections.keys().next().value;if(last)setActive(last);
}
function schedule(){if(!scheduled){scheduled=true;requestAnimationFrame(scrollspy);}clearTimeout(trimTimer);trimTimer=setTimeout(trimFrames,200);}
results.addEventListener('scroll',schedule,{passive:true});
function anchor(){if(results.scrollTop<=1)return null;const top=results.getBoundingClientRect().top;
 const nodes=results.querySelectorAll('.entry,.section-heading');for(const node of nodes){const rect=node.getBoundingClientRect();if(rect.bottom>top+1)return {node,top:rect.top};}return null;}
function restore(saved){if(saved?.node.isConnected)results.scrollTop+=saved.node.getBoundingClientRect().top-saved.top;}
function unload(card){const frame=loaded.get(card);if(!frame)return;frames.delete(frame.contentWindow);frame.remove();loaded.delete(card);card.classList.remove('loaded');}
function trimFrames(){if(loaded.size<=24)return;const r=results.getBoundingClientRect(),distance=r.height*3+600;
 for(const card of loaded.keys()){if(loaded.size<=24)break;const b=card.getBoundingClientRect();if(b.bottom<r.top-distance||b.top>r.bottom+distance)unload(card);}}
function load(card){if(loaded.has(card)||!card.isConnected)return;
 const frame=document.createElement('iframe');frame.setAttribute('sandbox','allow-scripts allow-same-origin');frame.setAttribute('scrolling','no');
 frame.title=card.dataset.title;frame.dataset.dictionary=card.dataset.dictionary;frame.style.height=card.style.height||'180px';
 card.append(frame);frames.set(frame.contentWindow,frame);loaded.set(card,frame);frame.src=card.dataset.src;
 frame.addEventListener('load',()=>{card.classList.add('loaded');schedule();});
}
const observer=new IntersectionObserver(entries=>{for(const entry of entries)if(entry.isIntersecting)load(entry.target);},{root:results,rootMargin:'500px 0px'});
function clear(){for(const frame of loaded.values())frame.remove();loaded.clear();frames.clear();observer.disconnect();sections.clear();results.replaceChildren();index.replaceChildren();active=null;results.scrollTop=0;}
function makeSection(data){
 const section=document.createElement('section');section.className='dictionary-section';section.id='dictionary-'+data.id;
 const heading=document.createElement('h2');heading.className='section-heading';heading.tabIndex=-1;
 const title=document.createElement('span');title.textContent=data.title;const count=document.createElement('span');count.className='count';heading.append(title,count);
 const body=document.createElement('div');body.className='definitions';section.append(heading,body);
 const button=document.createElement('button');button.className='index-link';button.title=data.title;button.dataset.dictionary=data.id;
 const label=document.createElement('span');label.className='index-title';label.textContent=data.title;const badge=document.createElement('span');badge.className='count';button.append(label,badge);
 button.setAttribute('aria-controls',section.id);button.onclick=()=>jump(data.id,true);
 button.addEventListener('keydown',event=>{const ids=[...sections.keys()];let n=ids.indexOf(data.id);
  if(event.key==='ArrowDown')n=(n+1)%ids.length;else if(event.key==='ArrowUp')n=(n+ids.length-1)%ids.length;else if(event.key==='Home')n=0;else if(event.key==='End')n=ids.length-1;else return;
  event.preventDefault();jump(ids[n],true);});
 const value={section,heading,title,count,body,button,label,badge,cards:new Map(),state:null};sections.set(data.id,value);results.append(section);index.append(button);return value;
}
function apply(state){
 appearance(state);window.setPinned(state.pinned);const changed=state.query!==currentQuery;
 if(changed){clear();currentQuery=state.query;query.value=state.query;window.currentQuery=state.query;}
 const saved=changed?null:anchor(),keep=new Set(state.sections.map(s=>s.id));
 for(const [id,v] of sections)if(!keep.has(id)){for(const card of v.cards.values()){observer.unobserve(card);unload(card);}v.section.remove();v.button.remove();sections.delete(id);}
 for(const data of state.sections){
  const v=sections.get(data.id)||makeSection(data);v.title.textContent=data.title;v.label.textContent=data.title;v.button.title=data.title;
  const pending=data.count===null;const count=pending?'…':String(data.count);v.count.textContent=pending?'検索待ち…':count+'件';v.badge.textContent=count;
  const entryIds=new Set(data.entries.map(e=>String(e.ordinal)));
  for(const [key,card] of v.cards)if(!entryIds.has(key)){observer.unobserve(card);unload(card);card.remove();v.cards.delete(key);}
  v.body.querySelector('.empty')?.remove();
  for(const entry of data.entries){const key=String(entry.ordinal);if(v.cards.has(key))continue;
   const card=document.createElement('article');card.className='entry';card.dataset.dictionary=data.id;card.dataset.src=entry.src;card.dataset.title=entry.headword;
   v.cards.set(key,card);v.body.append(card);observer.observe(card);
  }
  if(!data.entries.length){const empty=document.createElement('div');empty.className='empty';empty.textContent=pending?'順に検索しています…':'「'+state.query+'」に該当する項目はありません。';v.body.append(empty);}
 }
 let diagnostic=results.querySelector('.diagnostics');if(state.diagnostics.length){
  if(!diagnostic){diagnostic=document.createElement('details');diagnostic.className='diagnostics';diagnostic.append(document.createElement('summary'),document.createElement('pre'));results.append(diagnostic);}
  diagnostic.firstChild.textContent='辞書の診断 ('+state.diagnostics.length+')';diagnostic.lastChild.textContent=state.diagnostics.join('\n');
 }else diagnostic?.remove();
 if(!state.sections.length){let empty=results.querySelector('.no-dictionaries');if(!empty){empty=document.createElement('div');empty.className='empty no-dictionaries';empty.textContent='辞書が登録されていません。設定の「辞書」からMDXを追加してください。';results.append(empty);}}
 else results.querySelector('.no-dictionaries')?.remove();
 status.textContent=state.sections.filter(s=>s.count!==null).length+' / '+state.sections.length+' 辞書の検索が完了';
 restore(saved);if(changed&&state.selected&&sections.has(state.selected)){setActive(state.selected);const v=sections.get(state.selected);results.scrollTop=v.section.offsetTop-results.offsetTop;}else if(!active&&sections.size)setActive(sections.keys().next().value);
 schedule();
}
window.dictionaryView={apply,jump};
document.getElementById('pin').onclick=()=>send({action:'pin'});document.getElementById('close').onclick=()=>send({action:'close'});
query.addEventListener('keydown',e=>{if(e.key==='Enter')send({action:'lookup',query:e.target.value});});
document.getElementById('topbar').addEventListener('pointerdown',e=>{if(e.button===0&&!e.target.closest('button,input')){e.preventDefault();send({action:'drag'});}});
document.querySelectorAll('[data-resize]').forEach(edge=>edge.addEventListener('pointerdown',e=>{if(e.button===0){e.preventDefault();send({action:'resize',direction:edge.dataset.resize});}}));
function key(event){if(event.key==='Escape'){send({action:'close'});return;}const editing=event.target?.closest?.('input,textarea,select,[contenteditable=true]');if(editing)return;
 if(event.key==='F6'){event.preventDefault();if(index.contains(document.activeElement))results.focus();else sections.get(active)?.button.focus();}
 if(event.altKey&&(event.key==='ArrowDown'||event.key==='ArrowUp')){event.preventDefault();const ids=[...sections.keys()],n=ids.indexOf(active);jump(ids[Math.max(0,Math.min(ids.length-1,n+(event.key==='ArrowDown'?1:-1)))],true);}}
document.addEventListener('keydown',key);
window.addEventListener('message',event=>{const frame=frames.get(event.source);if(!frame)return;const m=event.data;if(!m||typeof m!=='object')return;
 if(m.action==='height'&&Number.isFinite(m.height)){const height=Math.max(24,Math.min(8000000,Math.ceil(m.height)));if(Math.abs(frame.getBoundingClientRect().height-height)<1)return;
  const saved=anchor();frame.style.height=height+'px';frame.parentElement.style.height=height+'px';frame.parentElement.classList.add('loaded');restore(saved);schedule();
 }else if(['entry','sound'].includes(m.action)&&m.dictionary_id===frame.dataset.dictionary)send(m);
 else if(m.action==='close')send({action:'close'});
 else if(m.action==='anchor'&&Number.isFinite(m.top)){results.scrollTop+=frame.getBoundingClientRect().top-results.getBoundingClientRect().top+m.top;}
 else if(m.action==='scroll'&&Number.isFinite(m.delta)){results.scrollTop+=Math.max(-results.clientHeight*3,Math.min(results.clientHeight*3,m.delta));}
 else if(m.action==='key'){const delta={ArrowDown:40,ArrowUp:-40,PageDown:results.clientHeight*.9,PageUp:-results.clientHeight*.9,' ':results.clientHeight*.9}[m.key];
  if(delta)results.scrollTop+=m.shift?-delta:delta;else if(m.key==='Home')results.scrollTop=0;else if(m.key==='End')results.scrollTop=results.scrollHeight;else if(m.key==='F6')sections.get(active)?.button.focus();}
});
})();
