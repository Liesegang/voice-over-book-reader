(async()=>{
const check=(ok,message)=>{if(!ok)throw Error(message);},b=globalThis.__vobrBook,near=(a,c)=>Math.abs(a-c)<.1;
const options={flow:'fixed',writing:'author',margins:[17,31,47,59],font_family:null,font_size:null,line_height:null,letter_spacing:null,ruby_scale:null,show_ruby:true,fixed_viewport:[320,480]};
const ids=b.testing.originalNodes.length;
await b.dispatch({type:'configure',revision:1,chapter:2,nodes:[],options});let events=b.drain();
check(!events.some(e=>e.type==='error'),JSON.stringify(events));check(events.some(e=>e.type==='pagination'&&e.pages===1),'Fixed count remains one');
const root=document.documentElement,box=root.getBoundingClientRect(),[t,r,d,l]=options.margins;
const aw=innerWidth-l-r,ah=innerHeight-t-d,scale=Math.min(aw/320,ah/480);
check(near(box.x,l+(aw-320*scale)/2)&&near(box.y,t+(ah-480*scale)/2)&&near(box.width,320*scale)&&near(box.height,480*scale),'Uniform centered author viewport in asymmetric margins');
check(box.left>=l-.1&&box.right<=innerWidth-r+.1&&box.top>=t-.1&&box.bottom<=innerHeight-d+.1,'Artwork fits footer-inclusive content bounds');
check(getComputedStyle(root).width==='320px'&&getComputedStyle(root).height==='480px','Internal author dimensions preserved');
const text=document.querySelector('a text'),source=b.testing.originalNodes.find(n=>n.parts&&n.dom.parentElement===text);
await b.dispatch({type:'rects',revision:1,ranges:[{chapter:2,node:source.node,start:0,end:1}]});
const glyph=b.drain().find(e=>e.type==='rects')?.rects[0];check(glyph?.width>0&&glyph.height>0,'Original source glyph geometry');
await b.dispatch({type:'hit_test',revision:1,x:glyph.x+glyph.width/2,y:glyph.y+glyph.height/2});
const hit=b.drain().find(e=>e.type==='hit')?.location;check(hit&&b.testing.originalNodes.some(n=>n.node===hit.node),'Hit remains an original source');
text.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true,clientX:glyph.x+glyph.width/2,clientY:glyph.y+glyph.height/2}));
const link=b.drain().find(e=>e.type==='link');check(link?.href.endsWith('#target')&&link.location,'Scaled link routes to native with original location');
await b.dispatch({type:'fragment',revision:1,fragment:'target'});check(b.drain().some(e=>e.type==='position'&&e.page===0),'Fixed fragment stays on source page');
for(const [align,revision] of [[-1,2],[1,3]]){
await b.dispatch({type:'configure',revision,chapter:2,nodes:[],options:{...options,fixed_horizontal_align:align}});b.drain();
const edge=root.getBoundingClientRect();check(near(edge.x,align<0?l:innerWidth-r-edge.width)&&near(edge.y,box.y)&&near(edge.width,box.width)&&near(edge.height,box.height),'Fixed artwork meets selected inside edge');
await b.dispatch({type:'rects',revision,ranges:[{chapter:2,node:source.node,start:0,end:1}]});const moved=b.drain().find(e=>e.type==='rects')?.rects[0];
text.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true,clientX:moved.x+moved.width/2,clientY:moved.y+moved.height/2}));const routed=b.drain().find(e=>e.type==='link');
check(routed?.href.endsWith('#target')&&routed.location?.node===hit.node,'Aligned source/link coordinates remain correct');
}
await b.dispatch({type:'configure',revision:4,chapter:2,nodes:[],options:{...options,margins:[0,0,0,0]}});events=b.drain();
const zero=root.getBoundingClientRect(),oldScale=Math.min(innerWidth/320,innerHeight/480);
check(near(zero.x,(innerWidth-320*oldScale)/2)&&near(zero.y,(innerHeight-480*oldScale)/2)&&near(zero.width,320*oldScale)&&near(zero.height,480*oldScale),'Zero margins retain previous transform');
check(b.testing.originalNodes.length===ids&&b.testing.originalNodes[source.node].dom===source.dom,'Margins never renumber source nodes');
console.log('NAV_REGRESSION PASS '+JSON.stringify({fixed:true,asymmetricMargins:true,footerFit:true,insideEdgeAlignment:true,zeroMarginCompatible:true,sourceIds:true,linkHit:true,pages:1,sourceNodes:ids,authorViewport:[320,480],scaledBounds:{x:box.x,y:box.y,width:box.width,height:box.height},scale}));
})().catch(error=>console.error('NAV_REGRESSION FAIL '+error.stack));
