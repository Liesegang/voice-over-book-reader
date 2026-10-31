// Book DOM only; original UTF-8 source identities survive pagination.
(() => {
    // Counter iframes share this implementation without child eval.
    const install = view => {
    if (view.__vobrBook) return view.__vobrBook;
    const doc = view.document;
    const getComputedStyle=view.getComputedStyle?.bind(view);
    const MutationObserver=view.MutationObserver,Highlight=view.Highlight,CSS=view.CSS;
    const setTimeout=view.setTimeout.bind(view),clearTimeout=view.clearTimeout.bind(view);
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
    let starts = [null], queue = [], resizeTimer, observer, applying=false, confirmed=false, invalidationPending=false,typographyStyle,measurementMarker;
    const nativeStyles=new WeakMap();
    const textMetrics=new WeakMap();
    const themedImages=new WeakMap(),imageWrites=new WeakMap();let scannedImagePixels=0;
    const emit = event => { queue.push({revision, ...event}); };
    const fail = error => emit({type:'error', message:String(error?.message ?? error)});
    const rectObject = r => ({x:r.x, y:r.y, width:r.width, height:r.height});
    const css = (el, props) => { for (const [key, value] of Object.entries(props)) el.style.setProperty(key, String(value), 'important');nativeStyles.set(el,el.getAttribute('style')); };
    const nodeAt = path => path.reduce((node, index) => node?.childNodes[index], doc.documentElement);
    const ownImageWrite=el=>{const write=imageWrites.get(el);return !!write&&el.getAttribute('src')===write.src&&el.getAttribute('srcset')===write.srcset;};
    const setImageSource=(image,source)=>{imageWrites.set(image,{src:source,srcset:image.getAttribute('srcset')});if(source===null)image.removeAttribute('src');else image.setAttribute('src',source);};
    const monochromeMask=(rgba,width,height)=>{
        let visible=0,transparent=0,binary=0,dark=0,light=0,border=0,borderDark=0,borderLight=0;
        for(let i=0,p=0;i<rgba.length;i+=4,p++){
            const r=rgba[i],g=rgba[i+1],b=rgba[i+2],a=rgba[i+3];if(a<16){transparent++;continue;}
            if(Math.max(r,g,b)-Math.min(r,g,b)>10)return null;
            visible++;const luma=Math.floor((r+g+b)/3),isDark=luma<=40,isLight=luma>=215;
            dark+=isDark;light+=isLight;binary+=isDark||isLight;
            const x=p%width,y=Math.floor(p/width);if(x===0||x+1===width||y===0||y+1===height){border++;borderDark+=isDark;borderLight+=isLight;}
        }
        if(!visible||binary*100<visible*65)return null;
        let lightPaper;
        if(transparent*100>=width*height*5){if(dark*100>=visible*65)lightPaper=true;else if(light*100>=visible*65)lightPaper=false;else return null;}
        else if(border&&borderLight*100>=border*80&&dark)lightPaper=true;
        else if(border&&borderDark*100>=border*80&&light)lightPaper=false;
        else return null;
        const mask=new Uint8Array(width*height);
        for(let i=0,p=0;i<rgba.length;i+=4,p++){const luma=Math.floor((rgba[i]+rgba[i+1]+rgba[i+2])/3);mask[p]=Math.floor(((lightPaper?255-luma:luma)*rgba[i+3]+127)/255);}
        return mask;
    };
    const sourceSelectorRisk=()=>{
        const seen=new Set();
        const rules=list=>{for(const rule of list){if(rule.selectorText&&/\[[^\]]*\b(?:src|srcset)\b/i.test(rule.selectorText))return true;if(rule.cssRules&&rules(rule.cssRules))return true;if(rule.styleSheet&&sheet(rule.styleSheet))return true;}return false;};
        const sheet=value=>{if(seen.has(value))return false;seen.add(value);try{return rules(value.cssRules);}catch{return true;}};
        return [...doc.styleSheets].some(sheet);
    };
    const textImageCandidate=(image,state)=>{
        if(image.getAttribute('srcset')||image.closest('picture'))return false;
        const name=[image.className,image.id,image.alt,state.original??''].join(' ');
        if(/photo|portrait|cover|illustration|screenshot|logo|banner|写真|挿絵|表紙/i.test(name))return false;
        const explicit=/gaiji|glyph|formula|equation|math|外字/i.test(name),style=getComputedStyle(image),box=image.getBoundingClientRect(),em=parseFloat(style.fontSize);
        if(!(em>0&&box.width>0&&box.height>0))return false;
        if(explicit)return box.width<=em*48&&box.height<=em*8;
        return !/\.(?:jpe?g|webp|avif|gif)(?:[?#]|$)/i.test(state.original??'')&&style.display.startsWith('inline')&&box.height<=em*2.5&&box.width<=em*4&&!!image.parentElement?.textContent.trim();
    };
    const restoreImage=async(image,state)=>{
        if(!state.applied)return;
        state.applied=false;state.color=null;
        if(image.style.getPropertyValue('background-color')==='transparent'){
            if(state.background)image.style.setProperty('background-color',state.background,state.backgroundPriority);
            else image.style.removeProperty('background-color');
            nativeStyles.set(image,image.getAttribute('style'));
        }
        if(image.getAttribute('src')===imageWrites.get(image)?.src){setImageSource(image,state.original);try{await image.decode();}catch{}}
    };
    const adaptImages=async token=>{
        const enabled=!fixed&&options.adapt_monochrome_images&&options.foreground&&!sourceSelectorRisk(),color=options.foreground?.join(',');
        for(const image of doc.querySelectorAll('img')){
            if(token!==serial)return;
            let state=themedImages.get(image);
            if(state&&((state.applied&&!ownImageWrite(image))||(!state.applied&&image.getAttribute('src')!==state.original))){await restoreImage(image,state);themedImages.delete(image);imageWrites.delete(image);state=null;}
            if(!enabled){if(state)await restoreImage(image,state);continue;}
            state??={original:image.getAttribute('src'),background:image.style.getPropertyValue('background-color'),backgroundPriority:image.style.getPropertyPriority('background-color'),mask:null,rejected:false,applied:false,color:null};
            themedImages.set(image,state);
            if(!textImageCandidate(image,state)){await restoreImage(image,state);continue;}
            if(state.rejected||state.applied&&state.color===color)continue;
            const width=image.naturalWidth,height=image.naturalHeight,pixels=width*height;
            if(!pixels||width>4096||height>4096||pixels>1048576)continue;
            const before=image.getBoundingClientRect(),canvas=doc.createElementNS('http://www.w3.org/1999/xhtml','canvas');canvas.width=width;canvas.height=height;
            try{
                const context=canvas.getContext('2d',{willReadFrequently:true});if(!context)continue;
                let data;
                if(!state.mask){
                    if(scannedImagePixels+pixels>8388608)continue;
                    scannedImagePixels+=pixels;context.drawImage(image,0,0);data=context.getImageData(0,0,width,height);
                    state.mask=monochromeMask(data.data,width,height);if(!state.mask){state.rejected=true;continue;}
                    state.width=width;state.height=height;
                }else if(state.width!==width||state.height!==height){await restoreImage(image,state);state.rejected=true;continue;}
                data??=context.createImageData(width,height);
                for(let p=0,i=0;p<state.mask.length;p++,i+=4){data.data[i]=options.foreground[0];data.data[i+1]=options.foreground[1];data.data[i+2]=options.foreground[2];data.data[i+3]=state.mask[p];}
                context.putImageData(data,0,0);setImageSource(image,canvas.toDataURL('image/png'));state.applied=true;state.color=color;css(image,{'background-color':'transparent'});
                await image.decode();
                if(token!==serial)return;
                const after=image.getBoundingClientRect();
                if(['x','y','width','height'].some(key=>Math.abs(before[key]-after[key])>.05)){await restoreImage(image,state);state.rejected=true;}
            }catch{if(token!==serial)return;await restoreImage(image,state);state.rejected=true;}
            finally{canvas.width=canvas.height=0;}
        }
    };
    // Original preorder IDs precede reader styles, including head/rt/rp.
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
                const parts=[];let text='',prefix=0,utf16_prefix=0;
                while(i<children.length&&isText(children[i])){
                    const dom=children[i];parts.push({dom,prefix,utf16_prefix});text+=dom.data;prefix+=byteLength(dom.data);utf16_prefix+=dom.data.length;i++;
                }
                i--;out.push({node:out.length,dom:child,text,parts});
            }
        };
        if(element)walk(element);return out;
    };
    const originalNodes=snapshotSourceNodes(doc?.documentElement);
    const fontChoice=globalThis.__vobrCreateFontChoice(view,originalNodes);
    const originalByDom=new WeakMap();
    for(const b of originalNodes){for(const part of b.parts??[{dom:b.dom,prefix:0}])originalByDom.set(part.dom,b);}
    let rootStyle=doc?.documentElement?.getAttribute('style')??null;
    let bodyStyle=doc?.body?.getAttribute('style')??null;
    const bind = nodes => {
        byId = new Map(); byNode = new WeakMap(); bindingList = [];
        const add=item=>{byId.set(item.node,item);for(const part of item.parts??[{dom:item.dom,prefix:0,utf16_prefix:0}])byNode.set(part.dom,{...item,prefix:part.prefix,utf16_prefix:part.utf16_prefix});bindingList.push(item);};
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
    const metrics=b=>{
        let value=textMetrics.get(b.parts);if(value)return value;
        const units=new Uint32Array(b.text.length+1),bytes=new Uint32Array(b.text.length+1);let unit=0,byte=0,index=1;
        for(const ch of b.text){const cp=ch.codePointAt(0);unit+=ch.length;byte+=cp<0x80?1:cp<0x800?2:cp<0x10000?3:4;units[index]=unit;bytes[index]=byte;index++;}
        value={units:units.subarray(0,index),bytes:bytes.subarray(0,index)};textMetrics.set(b.parts,value);return value;
    };
    const exactIndex=(values,value)=>{
        let lo=0,hi=values.length-1;
        while(lo<hi){const mid=(lo+hi)>>>1;if(values[mid]<value)lo=mid+1;else hi=mid;}
        if(values[lo]!==value)throw Error('Source offset splits a character or exceeds its node');return lo;
    };
    const locator = (node, offset) => {
        const b = byNode.get(node);
        if (!b) return null;
        if(!isText(node))return {chapter,node:b.node,offset:0};
        const m=metrics(b);return {chapter,node:b.node,offset:m.bytes[exactIndex(m.units,b.utf16_prefix+offset)]};
    };
    const resolveOffset=(b,bytes,end=false)=>{
        if(!b.parts)throw Error('Source is not text');
        const m=metrics(b),units=m.units[exactIndex(m.bytes,bytes)];
        for(let i=0;i<b.parts.length;i++){
            const part=b.parts[i],last=b.parts[i+1]?.prefix??m.bytes[m.bytes.length-1];
            if(bytes<last||(bytes===last&&(end||i===b.parts.length-1)))return [part.dom,units-part.utf16_prefix];
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
    const pageOfRect = r => {
        if (fixed) return 0;
        const [top,right,bottom,left] = options.margins;
        const coordinate = vertical ? (reverse ? view.innerHeight-bottom-r.bottom : r.top-top) : (reverse ? view.innerWidth-right-r.right : r.left-left);
        return Math.max(0,Math.min(pages-1,Math.floor((coordinate+page*stride+0.5)/stride)));
    };
    const charPage = (b,index,next) => {const r=rangeFor({chapter,node:b.node,start:index,end:next});const box=r.getClientRects()[0];return box ? pageOfRect(box) : 0;};
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
            const occupied=new Set(rects.map(pageOfRect)), bytes=metrics(b).bytes;
            for(const p of [...occupied].sort((a,b)=>a-b)){
                if(starts[p])continue;
                let lo=0,hi=bytes.length-1;
                while(lo<hi){const mid=Math.floor((lo+hi)/2);if(mid===bytes.length-1||charPage(b,bytes[mid],bytes[mid+1])>=p)hi=mid;else lo=mid+1;}
                if(lo<bytes.length-1 && charPage(b,bytes[lo],bytes[lo+1])===p) starts[p]={chapter,node:b.node,offset:bytes[lo]};
            }
        }
    };
    const marker=()=>{
        if(!options.measurement_marker){measurementMarker?.remove();measurementMarker=null;return null;}
        if(!measurementMarker){
            measurementMarker=doc.createElementNS('http://www.w3.org/1999/xhtml','div');measurementMarker.setAttribute('popover','manual');
            css(measurementMarker,{'position':'fixed','inset':'auto 2px 34px auto','width':'4px','height':'4px','padding':'0','margin':'0','border':'0','border-radius':'0','box-shadow':'none','opacity':'1','pointer-events':'none','transform':'none','filter':'none','mix-blend-mode':'normal','outline':'0'});
            root.append(measurementMarker);
            if(!measurementMarker.showPopover)throw Error('GPU measurement marker requires the Popover API');
            measurementMarker.showPopover();
        }
        const rgb=[16+((page&7)*32),16+(((page>>>3)&7)*32),128+((revision&3)*32)];css(measurementMarker,{'background-color':`rgb(${rgb.join(',')})`});return rgb;
    };
    const position = () => emit({type:'position',chapter,page,pages,start:starts[page]??null,marker:marker()});
    const invalidate=reason=>{if(invalidationPending)return;confirmed=false;invalidationPending=true;emit({type:'invalidated',reason});};
    const turn = (index, located=null) => {
        if(!confirmed)throw Error('Pagination is not confirmed for this revision');
        if (!Number.isInteger(index) || index<0 || index>=pages) throw Error('Page index outside confirmed pagination');
        page=index;
        if (!fixed) css(body,{'transform':`translate${vertical?'Y':'X'}(${(reverse?1:-1)*page*stride}px)`});
        if(located){marker();emit({type:'located',chapter,page,start:starts[page]??null,...located});}
        else position();
    };
    const ready = async token => {
        // Request used fonts before awaiting them.
        root.getBoundingClientRect(); await doc.fonts?.ready;
        await Promise.all([...doc.querySelectorAll('img')].map(async image=>{
            // Counters need intrinsic geometry, not decoded display bitmaps.
            // Failed or incomplete images retain the existing error path.
            if(options.pagination_only&&image.complete&&image.naturalWidth>0)return;
            try {if(image.decode)await image.decode();else if(!image.complete)await new Promise((resolve,reject)=>{image.addEventListener('load',resolve,{once:true});image.addEventListener('error',reject,{once:true});});}
            catch {fail(`Image could not be decoded: ${image.getAttribute('src')??''}`);}
        }));
        if(token!==serial)return;
        await adaptImages(token);if(token!==serial)return;
        // Ruby fonts can extend beyond their first line's allocated block edge.
        // Reserve only the measured overhang inside the existing page box;
        // page turns and speech highlights never repeat this layout step.
        if(!fixed&&options.show_ruby!==false){
            const [top,right,bottom,left]=options.margins;
            const edge=vertical?view.innerWidth-right:top;
            const available=vertical?view.innerWidth-left-right:view.innerHeight-top-bottom;
            let inset=0;
            for(let attempt=0;attempt<2;attempt++){
                let overhang=0;
                for(const annotation of doc.querySelectorAll('ruby rt')){
                    const range=doc.createRange();range.selectNodeContents(annotation);
                    for(const rect of range.getClientRects())if(rect.width>0&&rect.height>0)overhang=Math.max(overhang,vertical?rect.right-edge:edge-rect.top);
                }
                if(overhang<=.5)break;
                const next=Math.min(Math.ceil(inset+overhang),Math.max(0,available-1));
                if(next<=inset)break;
                inset=next;css(body,{'padding-block-start':`${inset}px`});
                root.getBoundingClientRect();
            }
        }
        root.getBoundingClientRect();if(token!==serial)return;
        const content = fixed ? 1 : vertical ? body.scrollHeight : body.scrollWidth;
        pages=fixed?1:Math.max(1,Math.ceil(content/stride));page=0;buildStarts();
        confirmed=true;applying=false;
        emit({type:'pagination',chapter,pages,page_starts:starts,ready:{fonts:true,images:true},writing_mode:getComputedStyle(body??root).writingMode});
        turn(0);
        observer?.disconnect();
        observer=new MutationObserver(records=>{
            if(applying)return;
            const changed=records.some(r=>!(r.type==='attributes'&&((r.attributeName==='style'&&nativeStyles.get(r.target)===r.target.getAttribute('style'))||(['src','srcset'].includes(r.attributeName)&&ownImageWrite(r.target)))));
            for(const r of records)if(r.type==='attributes'&&r.attributeName==='style'&&nativeStyles.get(r.target)!==r.target.getAttribute('style')){
                if(r.target===root)rootStyle=root.getAttribute('style');
                if(r.target===body)bodyStyle=body.getAttribute('style');
            }
            if(changed)invalidate('author_document_mutated');
        });
        observer.observe(root,{subtree:true,childList:true,characterData:true,attributes:true,attributeFilter:['class','style','src','srcset','href','width','height']});
    };
    const layout = async () => {
        const token=++serial;applying=true;confirmed=false;invalidationPending=false;observer?.disconnect();page=0;root=doc.documentElement;body=doc.body??root;fixed=options.flow==='fixed';
        if(rootStyle===null)root.removeAttribute('style');else root.setAttribute('style',rootStyle);
        if(body!==root){if(bodyStyle===null)body.removeAttribute('style');else body.setAttribute('style',bodyStyle);}
        css(root,{'overflow':'hidden','scroll-behavior':'auto'});
        let style=typographyStyle;
        if(!style){style=doc.createElementNS(root.namespaceURI==='http://www.w3.org/2000/svg'?root.namespaceURI:'http://www.w3.org/1999/xhtml','style');(doc.head??root).append(style);typographyStyle=style;}
        // Preserve author CSS; override only requested reader properties.
        const declarations=[];
        if(options.font_family)declarations.push(`font-family:${JSON.stringify(options.font_family)}`);
        if(options.font_size)declarations.push(`font-size:${options.font_size}px`);
        if(options.line_height)declarations.push(`line-height:${options.line_height}`);
        if(options.letter_spacing!=null)declarations.push(`letter-spacing:${options.letter_spacing}em`);
        if(options.writing!=='author')declarations.push(`writing-mode:${options.writing==='horizontal'?'horizontal-tb':'vertical-rl'}`);
        if(fixed)declarations.length=0;
        const rgb=value=>`rgb(${value.join(',')})`;
        const palette=[];
        if(options.background)palette.push(`html,body{background-color:${rgb(options.background)}!important}`);
        if(!fixed&&options.foreground)palette.push(`:where(body,body *):not(svg,svg *){color:${rgb(options.foreground)}!important}body :where(p,div,span,li,blockquote,td,th){background-color:transparent!important}`);
        const color=options.highlight_color??[43,112,235];
        style.textContent=`${options.default_css?`@layer vobr-default{${options.default_css}}`:''}html,body{${declarations.map(d=>d+'!important').join(';')}} rt{${!fixed&&options.ruby_scale?`font-size:${options.ruby_scale}em!important;`:''}}${!fixed&&options.show_ruby===false?'rt,rp,rtc{display:none!important}':''} ::highlight(vobr-speech){background:rgba(${color.join(',')},.33);color:inherit}::highlight(vobr-annotations){background:rgba(${color.join(',')},.2);text-decoration:underline;color:inherit}${palette.join('')}`;
        fontChoice.apply(options.font_family,fixed,el=>nativeStyles.set(el,el.getAttribute('style')));
        if(fixed){
            const [w,h]=options.fixed_viewport??[view.innerWidth,view.innerHeight];
            if(!(w>0&&h>0&&Number.isFinite(w)&&Number.isFinite(h)))throw Error('Fixed layout requires a positive author viewport');
            const [top,right,bottom,left]=options.margins,availableWidth=Math.max(1,view.innerWidth-left-right),availableHeight=Math.max(1,view.innerHeight-top-bottom);
            const scale=Math.min(availableWidth/w,availableHeight/h);
            const freeX=Math.max(0,availableWidth-w*scale),offsetX=options.fixed_horizontal_align<0?0:options.fixed_horizontal_align>0?freeX:freeX/2;
            css(root,{'width':`${w}px`,'height':`${h}px`,'transform-origin':'0 0','transform':`translate(${left+offsetX}px,${top+(availableHeight-h*scale)/2}px) scale(${scale})`});
            if(doc.body)css(body,{'margin':'0','width':`${w}px`,'height':`${h}px`});
            stride=1;
        }else{
            const wm=getComputedStyle(body).writingMode;vertical=wm.startsWith('vertical')||wm.startsWith('sideways');reverse=getComputedStyle(body).direction==='rtl';
            const [t,r,b,l]=options.margins, w=Math.max(1,view.innerWidth-l-r),h=Math.max(1,view.innerHeight-t-b),columns=options.columns??1;
            const gap=columns===2?(options.page_gap??0):(vertical?t+b:l+r),inline=vertical?h:w;
            stride=inline+gap;
            css(root,{'width':'100vw','height':'100vh','clip-path':columns===2?`inset(${t}px ${r}px ${b}px ${l}px)`:'none'});
            css(body,{'position':'absolute','top':`${t}px`,'left':`${l}px`,'margin':'0','padding':'0','box-sizing':'border-box','width':`${w}px`,'height':`${h}px`,'min-width':'0','max-width':'none','min-height':'0','max-height':'none','column-width':`${Math.max(1,(inline-gap*(columns-1))/columns)}px`,'column-gap':`${gap}px`,'column-fill':'auto','overflow':'visible','transform':'none','scroll-behavior':'auto'});
        }
        await ready(token);
    };
    const rubyBase=node=>{
        const ruby=(isText(node)?node.parentElement:node)?.closest?.('rt,rp,rtc')?.closest('ruby');if(!ruby)return null;
        const walk=doc.createTreeWalker(ruby,12);let n;
        while((n=walk.nextNode()))if(!n.parentElement.closest('rt,rp,rtc')){const i=n.data.search(/\S/u);if(i>=0){const loc=locator(n,i);if(loc)return loc;}}
        return null;
    };
    const hit = (x,y) => {
        const point=doc.caretPositionFromPoint?.(x,y);
        const range=point?null:doc.caretRangeFromPoint?.(x,y);
        let node=point?.offsetNode??range?.startContainer,offset=point?.offset??range?.startOffset??0;
        let result=node?(rubyBase(node)??locator(node,offset)):null;
        if(!result){const element=doc.elementFromPoint(x,y);const b=element?byNode.get(element):null;if(b)result={chapter,node:b.node,offset:0};}
        return result;
    };
    const selectionInfo=()=>{
        const s=doc.getSelection(),r=s?.rangeCount?s.getRangeAt(0):null,start=r?locator(r.startContainer,r.startOffset):null,end=r?locator(r.endContainer,r.endOffset):null;
        let text=s?.toString()??'';
        if(s?.rangeCount){const contents=s.getRangeAt(0).cloneContents();contents.querySelectorAll?.('rt,rp,rtc,script,style').forEach(el=>el.remove());text=contents.textContent??text;}
        if(start&&end&&start.node===end.node){const b=byId.get(start.node);if(b?.parts){const m=metrics(b);text=b.text.slice(m.units[exactIndex(m.bytes,Math.min(start.offset,end.offset))],m.units[exactIndex(m.bytes,Math.max(start.offset,end.offset))]);}}
        return {start,end,text};
    };
    const wordSegmenter=typeof Intl?.Segmenter==='function'?new Intl.Segmenter('ja',{granularity:'word'}):null;
    const wordAt=loc=>{
        const b=loc?byId.get(loc.node):null;if(!b?.parts)return '';
        const m=metrics(b),offset=m.units[exactIndex(m.bytes,loc.offset)];
        return wordSegmenter?.segment(b.text).containing(offset)?.segment??String.fromCodePoint(b.text.codePointAt(offset)??32).trim();
    };
    const pointer=(event,kind)=>{
        if(!options)return;
        try{const location=hit(event.clientX,event.clientY),s=selectionInfo();emit({type:'pointer',kind,x:event.clientX,y:event.clientY,location,text:s.text||wordAt(location),selection_start:s.start,selection_end:s.end});}catch(error){fail(error);}
    };
    const fragment=id=>{
        if(!id||id==='#'){turn(0);return;}
        id=id.startsWith('#')?id.slice(1):id;try{id=decodeURIComponent(id);}catch{}
        const element=doc.getElementById(id)??[...doc.getElementsByName(id)].find(el=>el.localName==='a');
        if(!element)throw Error('Link fragment does not exist in this document');
        const box=element.getClientRects()[0]??element.getBoundingClientRect();turn(pageOfRect(box));
    };
    const dispatch = async command => {
        try {
            if(command.type==='configure'){
                if(!Number.isSafeInteger(command.revision)||command.revision<0)throw Error('Invalid revision');
                if(command.revision<revision)return;
                revision=command.revision;chapter=command.chapter;options=command.options;
                if(!Array.isArray(options?.margins)||options.margins.length!==4||!options.margins.every(v=>Number.isFinite(v)&&v>=0))throw Error('Invalid margins');
                for(const name of ['font_size','line_height','ruby_scale'])if(options[name]!=null&&(!Number.isFinite(options[name])||options[name]<=0))throw Error(`Invalid ${name}`);
                if(options.letter_spacing!=null&&!Number.isFinite(options.letter_spacing))throw Error('Invalid letter spacing');
                if(options.columns!=null&&![1,2].includes(options.columns))throw Error('Invalid text column count');
                if(options.fixed_horizontal_align!=null&&![-1,0,1].includes(options.fixed_horizontal_align))throw Error('Invalid fixed page alignment');
                if(options.page_gap!=null&&(!Number.isFinite(options.page_gap)||options.page_gap<0))throw Error('Invalid page gap');
                for(const name of ['background','foreground','highlight_color'])if(options[name]!=null&&(!Array.isArray(options[name])||options[name].length!==3||!options[name].every(v=>Number.isInteger(v)&&v>=0&&v<=255)))throw Error(`Invalid ${name}`);
                bind(command.nodes);await layout();return;
            }
            if(command.revision!==revision)return;
            switch(command.type){
                case 'page':turn(command.index);break;
                case 'locate':{
                    const loc=command.location,request_id=command.request_id??0;
                    let target=null,reason=null;
                    try{
                        const b=byId.get(loc.node);
                        if(loc.chapter!==chapter||!b)throw Error('Unmapped source locator');
                        const r=doc.createRange();
                        if(b.parts){const m=metrics(b),index=exactIndex(m.bytes,loc.offset);r.setStart(...resolveOffset(b,loc.offset));r.setEnd(...resolveOffset(b,m.bytes[Math.min(index+1,m.bytes.length-1)],true));}
                        else r.selectNode(b.dom);
                        const box=[...r.getClientRects()].find(r=>r.width>0&&r.height>0);
                        if(!box)throw Error('Source locator has no visible geometry');
                        target=pageOfRect(box);
                    }catch(error){reason=String(error?.message??error);}
                    const fallback=Number.isInteger(command.fallback_page)?command.fallback_page:page;
                    const step=command.page_step===2?2:1;
                    const resolved=target??Math.max(0,Math.min(pages-1,fallback));
                    turn(Math.floor(resolved/step)*step,{request_id,location:loc,found:target!==null,reason});break;
                }
                case 'fragment':fragment(command.fragment);break;
                case 'hit_test':emit({type:'hit',location:hit(command.x,command.y)});break;
                case 'rects':emit({type:'rects',rects:command.ranges.flatMap(span=>[...rangeFor(span).getClientRects()].map(rectObject))});break;
                case 'highlight':{
                    if(!CSS?.highlights)throw Error('CSS Highlight API unavailable');
                    CSS.highlights.set('vobr-speech',new Highlight(...command.ranges.map(rangeFor)));break;
                }
                case 'annotations':{
                    if(!CSS?.highlights)throw Error('CSS Highlight API unavailable');
                    if(command.ranges.length)CSS.highlights.set('vobr-annotations',new Highlight(...command.ranges.map(rangeFor)));
                    else CSS.highlights.delete('vobr-annotations');break;
                }
                case 'clear_highlight':CSS.highlights?.delete('vobr-speech');break;
                case 'selection':{
                    emit({type:'selection',...selectionInfo()});break;
                }
                case 'clear_selection':doc.getSelection()?.removeAllRanges();break;
                case 'refresh':await layout();break;
                default:throw Error('Unknown navigator command');
            }
        }catch(error){fail(error);}
    };
    const bridge={dispatch,drain:()=>queue.splice(0),testing:{toUtf16,toUtf8,byteLength,snapshotSourceNodes,originalNodes,metrics,exactIndex,monochromeMask}};
    Object.defineProperty(view,'__vobrBook',{value:bridge,configurable:true});
    view.addEventListener?.('resize',()=>{if(!options)return;confirmed=false;clearTimeout(resizeTimer);resizeTimer=setTimeout(()=>invalidate('viewport_changed'),50);});
    doc?.fonts?.addEventListener?.('loadingdone',()=>{if(options&&!applying)invalidate('fonts_changed');});
    doc?.addEventListener?.('load',event=>{if(options&&!applying&&['img','image','link'].includes(event.target?.localName)&&!ownImageWrite(event.target))invalidate('resource_changed');},{capture:true});
    doc?.addEventListener?.('click',event=>{
        if(!options)return;
        const anchor=event.target?.closest?.('a,area'),raw=anchor?.getAttribute('href')??anchor?.getAttributeNS('http://www.w3.org/1999/xlink','href');
        if(raw!=null){event.preventDefault();emit({type:'link',href:new URL(raw,doc.baseURI).href,x:event.clientX,y:event.clientY,location:hit(event.clientX,event.clientY),user_gesture:!!event.isTrusted});return;}
        pointer(event,'click');
    },true);
    doc?.addEventListener?.('dblclick',event=>pointer(event,'double_click'),true);
    doc?.addEventListener?.('contextmenu',event=>{if(!options)return;event.preventDefault();pointer(event,'context_menu');},true);
    return bridge;
    };
    Object.defineProperty(globalThis,'__vobrInstallNavigator',{value:install,configurable:true});
    install(globalThis);
})();
