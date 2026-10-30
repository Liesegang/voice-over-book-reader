// Book DOM only. No web UI, filesystem access, network bridge, or author-text
// rewriting. Node IDs/paths arrive from the native source facade.
(() => {
    if (globalThis.__vobrBook) return;
    const doc = globalThis.document;
    const encoder = new TextEncoder();
    const byteLength = text => encoder.encode(text).length;
    const toUtf16 = (text, bytes) => {
        if (!Number.isInteger(bytes) || bytes < 0) throw Error('Invalid UTF-8 offset');
        let consumed = 0, index = 0;
        for (const ch of text) {
            if (consumed === bytes) return index;
            consumed += byteLength(ch); index += ch.length;
            if (consumed > bytes) throw Error('UTF-8 offset splits a character');
        }
        if (consumed === bytes) return index;
        throw Error('UTF-8 offset exceeds source node');
    };
    const toUtf8 = (text, index) => {
        if (!Number.isInteger(index) || index < 0 || index > text.length) throw Error('Invalid DOM offset');
        if (index && index < text.length && /[\uDC00-\uDFFF]/.test(text[index]) && /[\uD800-\uDBFF]/.test(text[index - 1])) throw Error('DOM offset splits a surrogate pair');
        return byteLength(text.slice(0, index));
    };
    let revision = 0, chapter = 0, serial = 0, page = 0, pages = 1;
    let vertical = false, reverse = false, stride = 1, fixed = false;
    let options, body, root, bindingList = [], byId = new Map(), byNode = new WeakMap();
    let starts = [null], queue = [], resizeTimer, observer, applying=false, confirmed=false;
    const nativeStyles=new WeakMap();
    const emit = event => { queue.push({revision, ...event}); };
    const fail = error => emit({type:'error', message:String(error?.message ?? error)});
    const rectObject = r => ({x:r.x, y:r.y, width:r.width, height:r.height});
    const css = (el, props) => { for (const [key, value] of Object.entries(props)) el.style.setProperty(key, String(value), 'important');nativeStyles.set(el,el.getAttribute('style')); };
    const nodeAt = path => path.reduce((node, index) => node?.childNodes[index], doc.documentElement);
    // Snapshot BEFORE native style injection. Core IDs count all original
    // elements/text from documentElement, including head/style/script/rt/rp.
    const isText=node=>node?.nodeType===3||node?.nodeType===4;
    const snapshotSourceNodes=element=>{
        const out=[];
        const walk=node=>{
            out.push({node:out.length,dom:node});
            const children=[...node.childNodes];
            for(let i=0;i<children.length;i++){
                const child=children[i];
                if(child.nodeType===1){walk(child);continue;}
                if(!isText(child))continue; // comments/PI break a run but have no ID
                const parts=[];let text='',prefix=0;
                while(i<children.length&&isText(children[i])){
                    const dom=children[i];parts.push({dom,prefix});text+=dom.data;prefix+=byteLength(dom.data);i++;
                }
                i--;out.push({node:out.length,dom:child,text,parts});
            }
        };
        if(element)walk(element);return out;
    };
    const originalNodes=snapshotSourceNodes(doc?.documentElement);
    const originalByDom=new WeakMap();
    for(const b of originalNodes){for(const part of b.parts??[{dom:b.dom,prefix:0}])originalByDom.set(part.dom,b);}
    const rootStyle=doc?.documentElement?.getAttribute('style')??null;
    const bodyStyle=doc?.body?.getAttribute('style')??null;
    const bind = nodes => {
        byId = new Map(); byNode = new WeakMap(); bindingList = [];
        const add=item=>{byId.set(item.node,item);for(const part of item.parts??[{dom:item.dom,prefix:0}])byNode.set(part.dom,{...item,prefix:part.prefix});bindingList.push(item);};
        if(!nodes.length){originalNodes.forEach(add);return;}
        for (const binding of nodes) {
            const node = nodeAt(binding.path), original=originalByDom.get(node);
            if (!original || (binding.text != null && original.text !== binding.text)) {
                fail(`Source node ${binding.node} differs from original DOM`); continue;
            }
            if (byId.has(binding.node)) {fail(`Duplicate source node ${binding.node}`); continue;}
            add({...original,node:binding.node});
        }
    };
    const locator = (node, offset) => {
        const b = byNode.get(node);
        if (!b) return null;
        return {chapter, node:b.node, offset:isText(node) ? b.prefix+toUtf8(node.data,offset) : 0};
    };
    const resolveOffset=(b,bytes,end=false)=>{
        if(!b.parts)throw Error('Source is not text');
        for(let i=0;i<b.parts.length;i++){
            const part=b.parts[i],last=part.prefix+byteLength(part.dom.data);
            if(bytes<last||(bytes===last&&(end||i===b.parts.length-1)))return [part.dom,toUtf16(part.dom.data,bytes-part.prefix)];
        }
        throw Error('Source offset exceeds text run');
    };
    const rangeFor = span => {
        if (span.chapter !== chapter) throw Error('Range belongs to another chapter');
        const b = byId.get(span.node); if (!b) throw Error(`Unmapped source node ${span.node}`);
        const r = doc.createRange();
        if (b.parts) {r.setStart(...resolveOffset(b,span.start));r.setEnd(...resolveOffset(b,span.end,true));}
        else r.selectNode(b.dom);
        return r;
    };
    const offsets = text => {const out=[0];let n=0;for(const ch of text){n+=ch.length;out.push(n);}return out;};
    const pageOfRect = r => {
        if (fixed) return 0;
        const [top,right,bottom,left] = options.margins;
        const coordinate = vertical ? (reverse ? innerHeight-bottom-r.bottom : r.top-top) : (reverse ? innerWidth-right-r.right : r.left-left);
        return Math.max(0,Math.min(pages-1,Math.floor((coordinate+page*stride+0.5)/stride)));
    };
    const charPage = (b,index,next) => {const r=rangeFor({chapter,node:b.node,start:toUtf8(b.text,index),end:toUtf8(b.text,next)});const box=r.getClientRects()[0];return box ? pageOfRect(box) : 0;};
    const buildStarts = () => {
        starts = Array(pages).fill(null);
        for (const b of bindingList) {
            if (!b.parts) {
                if(fixed || ['img','image','svg','object','video'].includes(b.dom.localName)){
                    const r=b.dom.getBoundingClientRect?.();if(r?.width && r.height){const p=pageOfRect(r);starts[p]??={chapter,node:b.node,offset:0};}
                }continue;
            }
            const range=rangeFor({chapter,node:b.node,start:0,end:byteLength(b.text)});
            const rects=[...range.getClientRects()].filter(r=>r.width>0 && r.height>0);
            if (!rects.length || !b.text.length || b.dom.parentElement?.closest('rt,rp,rtc,script,style,head')) continue;
            const occupied=new Set(rects.map(pageOfRect)), utf16=offsets(b.text);
            for(const p of [...occupied].sort((a,b)=>a-b)){
                if(starts[p])continue;
                let lo=0,hi=utf16.length-1;
                while(lo<hi){const mid=Math.floor((lo+hi)/2);if(charPage(b,utf16[mid],utf16[mid+1])<p)lo=mid+1;else hi=mid;}
                if(lo<utf16.length-1 && charPage(b,utf16[lo],utf16[lo+1])===p) starts[p]={chapter,node:b.node,offset:toUtf8(b.text,utf16[lo])};
            }
        }
    };
    const position = () => emit({type:'position',chapter,page,pages,start:starts[page]??null});
    const turn = index => {
        if(!confirmed)throw Error('Pagination is not confirmed for this revision');
        if (!Number.isInteger(index) || index<0 || index>=pages) throw Error('Page index outside confirmed pagination');
        page=index;
        if (!fixed) css(body,{'transform':`translate${vertical?'Y':'X'}(${(reverse?1:-1)*page*stride}px)`});
        position();
    };
    const ready = async token => {
        // Layout must request used fonts before waiting on FontFaceSet.ready.
        root.getBoundingClientRect(); await doc.fonts?.ready;
        await Promise.all([...doc.querySelectorAll('img')].map(async image=>{
            try {if(image.decode)await image.decode();else if(!image.complete)await new Promise((resolve,reject)=>{image.addEventListener('load',resolve,{once:true});image.addEventListener('error',reject,{once:true});});}
            catch {fail(`Image could not be decoded: ${image.getAttribute('src')??''}`);}
        }));
        if(token!==serial)return;
        await new Promise(requestAnimationFrame);if(token!==serial)return;
        const content = fixed ? 1 : vertical ? body.scrollHeight : body.scrollWidth;
        pages=fixed?1:Math.max(1,Math.ceil(content/stride));page=0;buildStarts();
        confirmed=true;applying=false;
        emit({type:'pagination',chapter,pages,page_starts:starts,ready:{fonts:true,images:true},writing_mode:getComputedStyle(body??root).writingMode});
        turn(0);
        observer?.disconnect();
        observer=new MutationObserver(records=>{
            if(applying)return;
            const changed=records.some(r=>!(r.type==='attributes'&&r.attributeName==='style'&&nativeStyles.get(r.target)===r.target.getAttribute('style')));
            if(changed){confirmed=false;emit({type:'invalidated',reason:'author_document_mutated'});}
        });
        observer.observe(root,{subtree:true,childList:true,characterData:true,attributes:true,attributeFilter:['class','style','src','href','width','height']});
    };
    const layout = async () => {
        const token=++serial;applying=true;confirmed=false;observer?.disconnect();page=0;root=doc.documentElement;body=doc.body??root;fixed=options.flow==='fixed';
        if(rootStyle===null)root.removeAttribute('style');else root.setAttribute('style',rootStyle);
        if(body!==root){if(bodyStyle===null)body.removeAttribute('style');else body.setAttribute('style',bodyStyle);}
        css(root,{'overflow':'hidden','scroll-behavior':'auto'});
        let style=doc.getElementById('__vobr_typography');
        if(!style){style=doc.createElementNS('http://www.w3.org/1999/xhtml','style');style.id='__vobr_typography';(doc.head??root).append(style);}
        // Reader controls override only the requested properties. Author rules,
        // selectors, custom properties, stylesheets, SVG and embedded fonts stay.
        const declarations=[];
        if(options.font_family)declarations.push(`font-family:${JSON.stringify(options.font_family)}`);
        if(options.font_size)declarations.push(`font-size:${options.font_size}px`);
        if(options.line_height)declarations.push(`line-height:${options.line_height}`);
        if(options.letter_spacing!=null)declarations.push(`letter-spacing:${options.letter_spacing}em`);
        if(options.writing!=='author')declarations.push(`writing-mode:${options.writing==='horizontal'?'horizontal-tb':'vertical-rl'}`);
        if(fixed)declarations.length=0;
        style.textContent=`html,body{${declarations.join(';')}} rt{${!fixed&&options.show_ruby===false?'visibility:hidden;':''}${!fixed&&options.ruby_scale?`font-size:${options.ruby_scale}em;`:''}} ::highlight(vobr-speech){background:#2b70eb55;color:inherit}`;
        if(fixed){
            const [w,h]=options.fixed_viewport??[innerWidth,innerHeight];
            if(!(w>0&&h>0))throw Error('Fixed layout requires a positive author viewport');
            const scale=Math.min(innerWidth/w,innerHeight/h);
            css(root,{'width':`${w}px`,'height':`${h}px`,'transform-origin':'0 0','transform':`translate(${(innerWidth-w*scale)/2}px,${(innerHeight-h*scale)/2}px) scale(${scale})`});
            if(doc.body)css(body,{'margin':'0','width':`${w}px`,'height':`${h}px`});
            stride=1;
        }else{
            const wm=getComputedStyle(body).writingMode;vertical=wm.startsWith('vertical')||wm.startsWith('sideways');reverse=getComputedStyle(body).direction==='rtl';
            const [t,r,b,l]=options.margins, w=Math.max(1,innerWidth-l-r),h=Math.max(1,innerHeight-t-b);
            stride=vertical?innerHeight:innerWidth;
            css(body,{'position':'absolute','top':`${t}px`,'left':`${l}px`,'margin':'0','padding':'0','box-sizing':'border-box','width':`${w}px`,'height':`${h}px`,'min-width':'0','max-width':'none','min-height':'0','max-height':'none','column-width':`${vertical?h:w}px`,'column-gap':`${vertical?t+b:l+r}px`,'column-fill':'auto','overflow':'visible','transform':'none','scroll-behavior':'auto'});
        }
        await ready(token);
    };
    const hit = (x,y) => {
        const point=doc.caretPositionFromPoint?.(x,y);
        const range=point?null:doc.caretRangeFromPoint?.(x,y);
        let node=point?.offsetNode??range?.startContainer,offset=point?.offset??range?.startOffset??0;
        let result=node?locator(node,offset):null;
        if(!result){const element=doc.elementFromPoint(x,y);const b=element?byNode.get(element):null;if(b)result={chapter,node:b.node,offset:0};}
        return result;
    };
    const dispatch = async command => {
        try {
            if(command.type==='configure'){
                if(!Number.isSafeInteger(command.revision)||command.revision<0)throw Error('Invalid revision');
                revision=command.revision;chapter=command.chapter;options=command.options;
                if(!options?.margins?.every(v=>Number.isFinite(v)&&v>=0))throw Error('Invalid margins');
                bind(command.nodes);await layout();return;
            }
            if(command.revision!==revision)return;
            switch(command.type){
                case 'page':turn(command.index);break;
                case 'locate':{
                    const loc=command.location,b=byId.get(loc.node);if(loc.chapter!==chapter||!b)throw Error('Unmapped source locator');
                    const r=doc.createRange();if(b.parts){const start=toUtf16(b.text,loc.offset),end=Math.min(b.text.length,start+(b.text.codePointAt(start)>0xFFFF?2:1));r.setStart(...resolveOffset(b,loc.offset));r.setEnd(...resolveOffset(b,toUtf8(b.text,end),true));}else r.selectNode(b.dom);
                    const box=r.getClientRects()[0];if(box)turn(pageOfRect(box));break;
                }
                case 'hit_test':emit({type:'hit',location:hit(command.x,command.y)});break;
                case 'rects':emit({type:'rects',rects:command.ranges.flatMap(span=>[...rangeFor(span).getClientRects()].map(rectObject))});break;
                case 'highlight':{
                    if(!globalThis.CSS?.highlights)throw Error('CSS Highlight API unavailable');
                    CSS.highlights.set('vobr-speech',new Highlight(...command.ranges.map(rangeFor)));break;
                }
                case 'clear_highlight':CSS.highlights?.delete('vobr-speech');break;
                case 'selection':{
                    const s=doc.getSelection(),start=s?.anchorNode?locator(s.anchorNode,s.anchorOffset):null,end=s?.focusNode?locator(s.focusNode,s.focusOffset):null;
                    let text=s?.rangeCount?s.getRangeAt(0).cloneContents().textContent??'':s?.toString()??'';
                    if(start&&end&&start.node===end.node){const b=byId.get(start.node);if(b?.parts)text=b.text.slice(toUtf16(b.text,Math.min(start.offset,end.offset)),toUtf16(b.text,Math.max(start.offset,end.offset)));}
                    emit({type:'selection',start,end,text});break;
                }
                case 'refresh':await layout();break;
                default:throw Error('Unknown navigator command');
            }
        }catch(error){fail(error);}
    };
    const bridge={dispatch,drain:()=>queue.splice(0),testing:{toUtf16,toUtf8,byteLength,snapshotSourceNodes,originalNodes}};
    Object.defineProperty(globalThis,'__vobrBook',{value:bridge,configurable:true});
    globalThis.addEventListener?.('resize',()=>{if(!options)return;confirmed=false;clearTimeout(resizeTimer);resizeTimer=setTimeout(()=>emit({type:'invalidated',reason:'viewport_changed'}),50);});
    doc?.fonts?.addEventListener?.('loadingdone',()=>{if(options&&!applying){confirmed=false;emit({type:'invalidated',reason:'fonts_changed'});}});
    doc?.addEventListener?.('load',event=>{if(options&&!applying&&['img','image','link'].includes(event.target?.localName)){confirmed=false;emit({type:'invalidated',reason:'resource_changed'});}},{capture:true});
})();
