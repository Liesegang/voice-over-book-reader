// Original-text CEF diagnostic. Off-page source glyphs are expected; only
// current-page glyphs and adjacent glyphs actually entering the view are tested.
(async()=>{
    const samples=[];
    for(const mode of ['horizontal','vertical'])for(const font of [24,64])for(const margin of [0,42])samples.push({name:`${mode}-${font}-m${margin}`,mode,font,margin,code:false});
    for(const mode of ['horizontal','vertical'])samples.push({name:`${mode}-64-code`,mode,font:64,margin:42,code:true});
    const selected=globalThis.__vobrViewportOnlyCase?samples.filter(test=>test.name===globalThis.__vobrViewportOnlyCase):samples;
    const results=[];
    for(const test of selected){
        const frame=document.createElement('iframe');
        frame.style.cssText='position:fixed;left:0;top:0;border:0;width:560px;height:680px;background:white';
        const loaded=new Promise((resolve,reject)=>{frame.onload=resolve;frame.onerror=reject;});
        frame.src='/case.xhtml?mode='+test.mode+'&code='+Number(test.code);document.body.append(frame);await loaded;
        const view=frame.contentWindow,doc=view.document,b=globalThis.__vobrInstallNavigator(view);
        const original=b.testing.originalNodes.map(node=>({node:node.node,dom:node.dom,parts:node.parts?.map(part=>({dom:part.dom,data:part.dom.data}))}));
        const options={flow:'reflowable',writing:test.mode==='vertical'?'vertical-rl':'horizontal',margins:[test.margin,test.margin,test.margin,test.margin],font_family:'Noto Serif CJK JP',font_size:test.font,line_height:1.65,letter_spacing:0,ruby_scale:.5,show_ruby:true,columns:1,default_css:globalThis.__vobrViewportDefaultCss};
        await b.dispatch({type:'configure',revision:1,chapter:0,nodes:[],options});
        const events=b.drain(),error=events.find(e=>e.type==='error');if(error)throw Error(JSON.stringify(error));
        const pagination=events.find(e=>e.type==='pagination');if(!pagination)throw Error('Missing pagination');
        const vertical=test.mode==='vertical',stride=vertical?view.innerHeight:view.innerWidth,content={left:test.margin,top:test.margin,right:view.innerWidth-test.margin,bottom:view.innerHeight-test.margin};
        const chars=[];
        for(const source of b.testing.originalNodes){
            if(!source.parts||source.dom.parentElement?.closest('head,style,script,rp'))continue;
            for(const part of source.parts){
                let offset=0;
                for(const ch of part.dom.data){
                    const start=offset;offset+=ch.length;if(/\s/u.test(ch))continue;
                    const range=doc.createRange();range.setStart(part.dom,start);range.setEnd(part.dom,offset);
                    const rect=range.getClientRects()[0];if(!rect||rect.width<=0||rect.height<=0)continue;
                    const page=Math.max(0,Math.min(pagination.pages-1,Math.floor(((vertical?rect.top:rect.left)-test.margin+.5)/stride)));
                    chars.push({range,page,node:source.node,offset:start,kind:part.dom.parentElement.closest('rt')?'ruby':part.dom.parentElement.closest('h1,h2,h3')?'heading':part.dom.parentElement.closest('pre,code')?'code':'body'});
                }
            }
        }
        const pages=[...new Set([0,Math.min(1,pagination.pages-1),Math.min(2,pagination.pages-1),pagination.pages-1])];
        const measured=[];
        for(const page of pages){
            await b.dispatch({type:'page',revision:1,index:page});b.drain();
            const result={page,currentGlyphs:0,outsideViewport:0,fullyOutsideViewport:0,outsideContent:0,adjacentInContent:0,adjacentInMargins:0,examples:[]};
            for(const char of chars){
                const r=char.range.getClientRects()[0];if(!r)continue;
                const intersectsViewport=r.right>.5&&r.bottom>.5&&r.left<view.innerWidth-.5&&r.top<view.innerHeight-.5;
                const intersectsContent=r.right>content.left+.5&&r.bottom>content.top+.5&&r.left<content.right-.5&&r.top<content.bottom-.5;
                const outsideViewport=r.left<-.75||r.top<-.75||r.right>view.innerWidth+.75||r.bottom>view.innerHeight+.75;
                const outsideContent=r.left<content.left-.75||r.top<content.top-.75||r.right>content.right+.75||r.bottom>content.bottom+.75;
                if(char.page===page){
                    result.currentGlyphs++;result.outsideViewport+=Number(outsideViewport);result.fullyOutsideViewport+=Number(!intersectsViewport);result.outsideContent+=Number(outsideContent);
                    if((outsideViewport||outsideContent)&&result.examples.length<6)result.examples.push({node:char.node,offset:char.offset,kind:char.kind,rect:{left:r.left,top:r.top,right:r.right,bottom:r.bottom},outsideViewport,outsideContent});
                }else if(intersectsContent)result.adjacentInContent++;else if(intersectsViewport)result.adjacentInMargins++;
            }
            measured.push(result);
        }
        const sourceStable=original.every((node,index)=>b.testing.originalNodes[index].node===node.node&&b.testing.originalNodes[index].dom===node.dom&&node.dom.isConnected&&(node.parts??[]).every(part=>part.dom.isConnected&&part.dom.data===part.data));
        const bodyBox=doc.body.getBoundingClientRect(),boxStable=Math.abs(bodyBox.width-(view.innerWidth-test.margin*2))<.1&&Math.abs(bodyBox.height-(view.innerHeight-test.margin*2))<.1;
        if(!sourceStable||!boxStable)throw Error('Source identities or border-box extent changed: '+test.name);
        results.push({...test,viewport:[view.innerWidth,view.innerHeight],pages:pagination.pages,sourceGlyphs:chars.length,measured,sourceStable,boxStable,rubyGuard:doc.body.style.getPropertyValue('padding-block-start'),viewportClear:measured.every(page=>page.outsideViewport===0&&page.adjacentInContent===0),contentClear:measured.every(page=>page.outsideContent===0&&page.adjacentInContent===0)});
        frame.remove();
    }
    const fixedFrame=document.createElement('iframe');fixedFrame.style.cssText='position:fixed;inset:0;border:0;width:560px;height:680px';
    const fixedLoaded=new Promise(resolve=>fixedFrame.onload=resolve);fixedFrame.src='/case.xhtml?mode=vertical&fixed=1';document.body.append(fixedFrame);await fixedLoaded;
    const fixedView=fixedFrame.contentWindow,fixedDoc=fixedView.document,fixedBridge=globalThis.__vobrInstallNavigator(fixedView),fixedNodes=fixedBridge.testing.originalNodes.map(node=>node.dom),beforePadding=fixedView.getComputedStyle(fixedDoc.body).paddingTop;
    await fixedBridge.dispatch({type:'configure',revision:1,chapter:0,nodes:[],options:{flow:'fixed',writing:'author',margins:[0,0,0,0],fixed_viewport:[560,680],font_family:'Noto Serif CJK JP',font_size:64,line_height:1.65,ruby_scale:.5,show_ruby:true,default_css:globalThis.__vobrViewportDefaultCss}});
    const fixedEvents=fixedBridge.drain(),fixedUnchanged=!fixedEvents.some(event=>event.type==='error')&&fixedView.getComputedStyle(fixedDoc.body).paddingTop===beforePadding&&!fixedDoc.body.style.getPropertyValue('padding-block-start')&&fixedNodes.every((node,index)=>node===fixedBridge.testing.originalNodes[index].dom&&node.isConnected);
    fixedFrame.remove();if(!fixedUnchanged)throw Error('Fixed author padding or original nodes changed');
    const report={cases:results,fixedUnchanged,allViewportClear:results.every(test=>test.viewportClear),allContentClear:results.every(test=>test.contentClear),scope:'Original Japanese prose, headings, ruby and native sample-style code. Range geometry is not a pixel-ink measurement.'};
    await fetch('/result',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(report)});
    console.log('PROSE_VIEWPORT_COMPLETE '+JSON.stringify({allViewportClear:report.allViewportClear,allContentClear:report.allContentClear,cases:results.length}));
})().catch(async error=>{console.error('PROSE_VIEWPORT_FAIL '+error.stack);try{await fetch('/result',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({error:String(error.stack)})});}catch{}});
