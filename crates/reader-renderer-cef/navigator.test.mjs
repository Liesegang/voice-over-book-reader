import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
await import('./font_choice.test.mjs');
const context={TextEncoder,console,setTimeout,clearTimeout};
vm.createContext(context);
vm.runInContext(fs.readFileSync(new URL('./src/font_choice.js',import.meta.url),'utf8'),context);
vm.runInContext(fs.readFileSync(new URL('./src/navigator.js',import.meta.url),'utf8'),context);
const {toUtf16,toUtf8}=context.__vobrBook.testing;
assert.equal(typeof context.__vobrInstallNavigator,'function');
const childView={TextEncoder,setTimeout,clearTimeout};
const childBridge=context.__vobrInstallNavigator(childView);
assert.equal(childView.__vobrBook,childBridge);
assert.notEqual(childBridge,context.__vobrBook,'counter iframe state is independent of the visible book');
assert.equal(context.__vobrInstallNavigator(childView),childBridge,'installing twice never adds duplicate state');
const element=(name,...childNodes)=>({nodeType:1,localName:name,childNodes});
const t=data=>({nodeType:3,data,childNodes:[]});
const c=data=>({nodeType:4,data,childNodes:[]});
const comment={nodeType:8,childNodes:[]};
const tree=element('html',element('head',element('style',t('x{}'))),element('body',element('p',t('漢'),c('😀'),t('字'),comment,t('文')),element('ruby',t('頁'),element('rt',t('ページ')))));
const nodes=context.__vobrBook.testing.snapshotSourceNodes(tree);
assert.deepEqual(Array.from(nodes,n=>n.parts?n.text:n.dom.localName),['html','head','style','x{}','body','p','漢😀字','文','ruby','頁','rt','ページ']);
assert.deepEqual(Array.from(nodes[6].parts,p=>p.prefix),[0,3,7]);
assert.deepEqual(Array.from(nodes[6].parts,p=>p.utf16_prefix),[0,1,3]);
const {metrics,exactIndex}=context.__vobrBook.testing;
const mapped=metrics(nodes[6]);
assert.deepEqual(Array.from(mapped.units),[0,1,3,4]);
assert.deepEqual(Array.from(mapped.bytes),[0,3,7,10]);
assert.equal(metrics(nodes[6]),mapped,'long source nodes reuse their UTF conversion table');
assert.equal(exactIndex(mapped.bytes,7),2);
assert.throws(()=>exactIndex(mapped.bytes,4));
assert.throws(()=>exactIndex(mapped.bytes,11));
const mask=context.__vobrBook.testing.monochromeMask;
const tile=new Uint8ClampedArray(4*4*4).fill(255);
for(const p of [5,6,9,10])tile.set([0,0,0,255],p*4);
const alpha=mask(tile,4,4);assert.equal(alpha[0],0);assert.equal(alpha[5],255);
const transparent=new Uint8ClampedArray(4*4*4);transparent.set([255,255,255,255],5*4);
assert.equal(mask(transparent,4,4)[5],255,'white on transparent retains coverage');
tile.set([0,0,255,255],5*4);assert.equal(mask(tile,4,4),null,'colored artwork is unchanged');
const gradient=new Uint8ClampedArray(16*16*4);
for(let p=0;p<256;p++)gradient.set([p,p,p,255],p*4);
assert.equal(mask(gradient,16,16),null,'continuous-tone grayscale photos are unchanged');
const text='漢😀e\u0301𠮷文';
const pairs=[[0,0],[3,1],[7,3],[8,4],[10,5],[14,7],[17,8]];
for(const [bytes,units] of pairs){assert.equal(toUtf16(text,bytes),units);assert.equal(toUtf8(text,units),bytes);}
for(const offset of [1,2,4,5,6,9,11,12,13,15,16,18])assert.throws(()=>toUtf16(text,offset));
assert.throws(()=>toUtf8(text,2));assert.throws(()=>toUtf8(text,6));
await context.__vobrBook.dispatch({type:'page',revision:987,index:1});
assert.equal(context.__vobrBook.drain().length,0,'old revisions must not move pages');
await context.__vobrBook.dispatch({type:'configure',revision:1,chapter:0,nodes:[],options:{margins:[1,2,3]}});
assert.match(context.__vobrBook.drain()[0].message,/Invalid margins/);
await context.__vobrBook.dispatch({type:'configure',revision:2,chapter:0,nodes:[],options:{margins:[0,0,0,0],fixed_horizontal_align:2}});
assert.match(context.__vobrBook.drain()[0].message,/Invalid fixed page alignment/);
console.log('navigator protocol: UTF-8/UTF-16, astral/combining characters, boundaries and revision checks passed');

// Exercise real dispatch and layout with deterministic DOM geometry, without
// launching a browser. Source offsets 0/3 are on page 0, offset 6 is on page 1.
function documentFixture({imageOnly=false,broken=false,authorFamily='JA, serif',bodyFamily=authorFamily,faces=[]}={}){
    const properties=new WeakMap();
    const make=(name,...children)=>{
        const node=element(name,...children),attrs=new Map(),values=new Map();
        properties.set(node,values);
        const priorities=new Map();
        node.style={setProperty:(key,value,priority='')=>{values.set(key,value);priorities.set(key,priority);attrs.set('style',[...values].join(';'));},getPropertyValue:key=>values.get(key)??'',getPropertyPriority:key=>priorities.get(key)??'',removeProperty:key=>{values.delete(key);priorities.delete(key);attrs.set('style',[...values].join(';'));}};
        node.getAttribute=key=>attrs.get(key)??null;
        node.removeAttribute=key=>attrs.delete(key);
        node.setAttribute=(key,value)=>attrs.set(key,value);
        node.append=child=>{node.childNodes.push(child);child.parentElement=node;};
        node.showPopover=()=>{};
        node.remove=()=>{if(node.parentElement)node.parentElement.childNodes=node.parentElement.childNodes.filter(child=>child!==node);};
        node.closest=()=>null;
        node.getBoundingClientRect=()=>({x:0,y:0,left:0,top:0,right:300,bottom:300,width:300,height:300});
        for(const child of children)child.parentElement=node;
        return node;
    };
    const text=t('甲乙丙'),image=make('img');let decodes=0;
    image.complete=true;image.naturalWidth=broken?0:300;image.naturalHeight=300;
    image.decode=async()=>{decodes++;if(broken)throw Error('broken image');};
    const head=make('head'),body=make('body',imageOnly?image:make('p',text)),root=make('html',head,body);
    body.family=bodyFamily;if(text.parentElement){text.parentElement.family=authorFamily;text.parentElement.style.setProperty('font-weight','400');}
    body.scrollWidth=1000;body.scrollHeight=400;
    const rect=index=>{
        const transform=properties.get(body).get('transform')??'';
        const shifted=Number(/translateX\((-?[\d.]+)px\)/.exec(transform)?.[1]??0);
        const x=(index<2?20:520)+shifted;
        return {x,y:20,left:x,right:x+12,top:20,bottom:40,width:12,height:20};
    };
    const fonts=faces.map(family=>({family}));Object.assign(fonts,{ready:Promise.resolve(),addEventListener(){}});
    const doc={documentElement:root,head,body,
        fonts,
        querySelectorAll:selector=>selector==='img'&&imageOnly?[image]:[],
        createElementNS:(_,name)=>make(name),addEventListener(){},
        createRange(){let a=0,b=1,selected=null;return {
            setStart:(_,offset)=>{a=offset;},setEnd:(_,offset)=>{b=offset;},selectNode:node=>{selected=node;},
            getClientRects:()=>selected?[selected.getBoundingClientRect()]:Array.from({length:Math.max(1,b-a)},(_,i)=>rect(a+i)),
        };},
    };
    const family=element=>element?.style.getPropertyValue('font-family')||element?.family||(element?.parentElement?family(element.parentElement):'serif');
    const view={document:doc,innerWidth:500,innerHeight:400,setTimeout,clearTimeout,addEventListener(){},
        getComputedStyle:element=>({writingMode:'horizontal-tb',direction:'ltr',fontFamily:family(element)}),
        MutationObserver:class{observe(){}disconnect(){}},CSS:{highlights:new Map()},Highlight:class{constructor(...ranges){this.ranges=ranges;}},
    };
    return {bridge:context.__vobrInstallNavigator(view),decodes:()=>decodes,family:()=>family(text.parentElement),weight:()=>text.parentElement?.style.getPropertyValue('font-weight'),source:()=>text,
        marker:()=>properties.get(root.childNodes.find(node=>node.getAttribute?.('popover')==='manual'))?.get('background-color')};
}
const fixture=documentFixture();
await fixture.bridge.dispatch({type:'configure',revision:30,chapter:0,nodes:[],options:{margins:[0,0,0,0],flow:'reflowable',writing:'horizontal',font_family:'Noto Serif CJK JP',measurement_marker:true}});
let replies=fixture.bridge.drain();
assert.equal(replies.find(e=>e.type==='pagination').pages,2);
assert.equal(fixture.marker(),'rgb(16,16,192)');
assert.equal(fixture.family(),'"Noto Serif CJK JP", JA, serif');
const selectedFamily=fixture.family(),selectedWeight=fixture.weight(),originalText=fixture.source();
const highlightedRange={chapter:0,node:4,start:0,end:3};
await fixture.bridge.dispatch({type:'rects',revision:30,ranges:[highlightedRange]});
const beforeHighlight=fixture.bridge.drain()[0].rects;
await fixture.bridge.dispatch({type:'highlight',revision:30,ranges:[highlightedRange]});
assert.equal(fixture.bridge.drain().length,0);
await fixture.bridge.dispatch({type:'rects',revision:30,ranges:[highlightedRange]});
assert.deepEqual(fixture.bridge.drain()[0].rects,beforeHighlight,'highlight commands preserve the source Range geometry in this deterministic DOM');
assert.equal(fixture.family(),selectedFamily);assert.equal(fixture.weight(),selectedWeight);assert.equal(fixture.source(),originalText,'highlighting never replaces source text nodes');
// The counter uses the same factory with its own iframe view. Neither computed
// families nor the iframe FontFaceSet may be borrowed from the visible book.
const counterFont=documentFixture({authorFamily:'Counter Book, serif'});
await counterFont.bridge.dispatch({type:'configure',revision:50,chapter:8,nodes:[],options:{margins:[0,0,0,0],flow:'reflowable',writing:'horizontal',font_family:'Noto Sans CJK JP',pagination_only:true}});
assert.ok(counterFont.bridge.drain().some(e=>e.type==='pagination'));
assert.equal(counterFont.family(),'"Noto Sans CJK JP", Counter Book, serif');assert.equal(fixture.family(),selectedFamily);
const embeddedCounter=documentFixture({authorFamily:'Counter Embedded Face',bodyFamily:'serif',faces:['Counter Embedded Face']});
await embeddedCounter.bridge.dispatch({type:'configure',revision:51,chapter:9,nodes:[],options:{margins:[0,0,0,0],flow:'reflowable',writing:'horizontal',font_family:'Noto Sans CJK JP',pagination_only:true}});
assert.ok(embeddedCounter.bridge.drain().some(e=>e.type==='pagination'));
assert.equal(embeddedCounter.family(),'Counter Embedded Face','an iframe-only embedded symbol face remains authored');
for(const [request_id,offset] of [[1,0],[2,3],[3,0],[4,3]]){
    await fixture.bridge.dispatch({type:'locate',revision:30,request_id,location:{chapter:0,node:4,offset},fallback_page:0});
    const result=fixture.bridge.drain();assert.equal(result.length,1);
    assert.equal(result[0].type,'located');assert.equal(result[0].request_id,request_id);assert.equal(result[0].found,true);assert.equal(result[0].page,0);
}
await fixture.bridge.dispatch({type:'locate',revision:30,request_id:5,location:{chapter:0,node:4,offset:6},fallback_page:0});
assert.equal(fixture.bridge.drain()[0].page,1);
assert.equal(fixture.marker(),'rgb(48,16,192)','source-follow page changes update the painted marker without an extra Position reply');
await fixture.bridge.dispatch({type:'locate',revision:30,request_id:6,location:{chapter:0,node:4,offset:6},fallback_page:0,page_step:2});
assert.equal(fixture.bridge.drain()[0].page,0,'the leading spread pane stays aligned when speech is on its second page');
assert.equal(fixture.marker(),'rgb(16,16,192)','the marker follows the confirmed leading page of a prose spread');
await fixture.bridge.dispatch({type:'locate',revision:30,request_id:7,location:{chapter:0,node:4294967295,offset:0},fallback_page:1});
replies=fixture.bridge.drain();assert.equal(replies[0].type,'located');assert.equal(replies[0].found,false);assert.equal(replies[0].page,1);assert.equal(replies[0].start.offset,6);
assert.match(replies[0].reason,/Unmapped source locator/);
await fixture.bridge.dispatch({type:'page',revision:30,index:0});
assert.equal(fixture.bridge.drain()[0].page,0,'a failed legacy locator must not disable subsequent page turns');
assert.equal(fixture.family(),selectedFamily);assert.equal(fixture.weight(),selectedWeight,'Locate and Page never change author font weight');
await fixture.bridge.dispatch({type:'configure',revision:31,chapter:1,nodes:[],options:{margins:[0,0,0,0],flow:'reflowable',writing:'horizontal',font_family:'Noto Serif CJK JP'}});
fixture.bridge.drain();
await fixture.bridge.dispatch({type:'locate',revision:30,request_id:80,location:{chapter:0,node:4,offset:6},fallback_page:1});
assert.equal(fixture.bridge.drain().length,0,'a previous chapter/revision cannot locate the newly configured document');
const imageFixture=documentFixture({imageOnly:true});
await imageFixture.bridge.dispatch({type:'configure',revision:40,chapter:2,nodes:[],options:{margins:[0,0,0,0],flow:'fixed',pagination_only:true}});
imageFixture.bridge.drain();assert.equal(imageFixture.decodes(),0,'confirmed image dimensions do not require counter bitmap decoding');
await imageFixture.bridge.dispatch({type:'locate',revision:40,request_id:20,location:{chapter:2,node:4294967295,offset:0},fallback_page:19});
replies=imageFixture.bridge.drain();assert.equal(replies[0].type,'located');assert.equal(replies[0].found,false);assert.equal(replies[0].page,0);assert.ok(replies[0].start,'image-only pages retain a real DOM page anchor');
const brokenFixture=documentFixture({imageOnly:true,broken:true});
await brokenFixture.bridge.dispatch({type:'configure',revision:41,chapter:0,nodes:[],options:{margins:[0,0,0,0],flow:'fixed',pagination_only:true}});
assert.equal(brokenFixture.decodes(),1);assert.ok(brokenFixture.bridge.drain().some(e=>e.type==='error'&&/decoded/.test(e.message)),'real image load failures remain errors');
console.log('navigator follow: correlated replies, same-page boundaries, spreads, legacy/image fallback and counter decode checks passed');
