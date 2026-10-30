(()=>{'use strict';const id=window.dictionaryIdentity;const send=m=>parent.postMessage(m,'*');
let pending=false,lastHeight=0;
function size(){pending=false;const body=document.body;if(!body)return;
 const height=Math.ceil(Math.max(body.scrollHeight,body.offsetHeight,body.getBoundingClientRect().height));
 if(height!==lastHeight){lastHeight=height;send({action:'height',height});}}
function schedule(){if(!pending){pending=true;requestAnimationFrame(size);}}
new ResizeObserver(schedule).observe(document.body);new MutationObserver(schedule).observe(document.body,{subtree:true,childList:true,attributes:true,characterData:true});
window.addEventListener('load',schedule);document.fonts?.ready.then(schedule);setTimeout(schedule,50);
function editing(target){return target?.closest?.('input,textarea,select,[contenteditable=true]');}
document.addEventListener('keydown',e=>{if(e.key==='Escape'){send({action:'close'});return;}if(editing(e.target)||e.ctrlKey||e.altKey||e.metaKey)return;
 if(['ArrowDown','ArrowUp','PageDown','PageUp','Home','End',' ','F6'].includes(e.key)){e.preventDefault();send({action:'key',key:e.key,shift:e.shiftKey});}});
document.addEventListener('wheel',e=>{if(e.ctrlKey||Math.abs(e.deltaX)>Math.abs(e.deltaY)||!e.deltaY)return;
 let target=e.target instanceof Element?e.target:null;
 // Preserve intentionally scrollable dictionary widgets, then chain at their edge.
 while(target&&target!==document.body){const css=getComputedStyle(target);if(/auto|scroll/.test(css.overflowY)&&target.scrollHeight>target.clientHeight+1&&((e.deltaY>0&&target.scrollTop+target.clientHeight<target.scrollHeight-1)||(e.deltaY<0&&target.scrollTop>0)))return;target=target.parentElement;}
 e.preventDefault();send({action:'scroll',delta:e.deltaY*(e.deltaMode===1?20:e.deltaMode===2?window.innerHeight:1)});
},{passive:false});
document.addEventListener('click',e=>{const a=e.target.closest?.('a');if(!a)return;const raw=a.getAttribute('href')||'';
 if(raw.startsWith('entry://')||raw.startsWith('bword:')){e.preventDefault();let query=raw.startsWith('entry://')?raw.slice(8):raw.slice(6);try{query=decodeURIComponent(query);}catch{}send({action:'entry',dictionary_id:id,query});}
 else if(raw.startsWith('sound://')){e.preventDefault();send({action:'sound',dictionary_id:id,key:raw});}
 else if(raw.startsWith('#')){let key=raw.slice(1);try{key=decodeURIComponent(key);}catch{}const target=document.getElementById(key)||document.getElementsByName(key)[0];if(target){e.preventDefault();send({action:'anchor',top:target.getBoundingClientRect().top+window.scrollY});}}
});
schedule();
})();
