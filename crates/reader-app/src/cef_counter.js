function countChapters(items) {
    globalThis.__vobrCountResults=[];
    globalThis.__vobrCountErrors=[];
    const pending=[],seen=new Set();
    let active=0,next=0;
    const launch=()=>{
        while(active<3&&next<pending.length){
        // A fixed-layout spine item is one source page even when the reader
        // displays it beside another item. Do not fetch/rasterize its artwork
        // just to rediscover the publisher's declared page count.
        const item=pending[next++];
        if(item.options.flow==='fixed'){
            globalThis.__vobrCountResults.push({chapter:item.chapter,starts:[null]});
            continue;
        }
        active++;
        const frame=document.createElement('iframe');
        frame.style.cssText=`position:absolute;left:0;top:0;width:${item.width}px;height:${item.height}px;border:0;`;
        let removed=false;
        const finish=()=>{if(removed)return;removed=true;frame.remove();active--;launch();};
        frame.onload=()=>{
            try {
                const bridge=globalThis.__vobrInstallNavigator(frame.contentWindow);
                bridge.dispatch({type:'configure',revision:1,chapter:item.chapter,nodes:[],options:{...item.options,pagination_only:true}});
                const started=performance.now();
                const poll=()=>{
                    if(removed)return;
                    try {
                        const events=bridge.drain();
                        for(const event of events){
                            if(event.type==='error')throw Error(event.message);
                            if(event.type==='pagination'){
                                globalThis.__vobrCountResults.push({chapter:item.chapter,starts:event.page_starts});
                                finish();return;
                            }
                        }
                        if(performance.now()-started>30000)throw Error('Chapter pagination timed out');
                        setTimeout(poll,4);
                    } catch(error){globalThis.__vobrCountErrors.push(String(error));finish();}
                };
                poll();
            } catch(error){globalThis.__vobrCountErrors.push(String(error));finish();}
        };
        frame.onerror=()=>{globalThis.__vobrCountErrors.push('Chapter resource failed');finish();};
        frame.src=item.url;
        document.body.append(frame);
        }
        if(next===pending.length){pending.length=0;next=0;}
    };
    globalThis.__vobrEnqueueCountItems=items=>{
        for(const item of items)if(!seen.has(item.chapter)){seen.add(item.chapter);pending.push(item);}
        launch();
        return true;
    };
    globalThis.__vobrEnqueueCountItems(items);
    return true;
}
