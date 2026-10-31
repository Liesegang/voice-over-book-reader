function countChapters(items) {
    globalThis.__vobrCountResults=[];
    globalThis.__vobrCountErrors=[];
    let next=0;
    const launch=()=>{
        // A fixed-layout spine item is one source page even when the reader
        // displays it beside another item. Do not fetch/rasterize its artwork
        // just to rediscover the publisher's declared page count.
        while(next<items.length&&items[next].options.flow==='fixed'){
            const item=items[next++];
            globalThis.__vobrCountResults.push({chapter:item.chapter,starts:[null]});
        }
        if(next>=items.length)return;
        const item=items[next++];
        const frame=document.createElement('iframe');
        frame.style.cssText=`position:absolute;left:0;top:0;width:${item.width}px;height:${item.height}px;border:0;`;
        frame.onload=()=>{
            try {
                const bridge=globalThis.__vobrInstallNavigator(frame.contentWindow);
                bridge.dispatch({type:'configure',revision:1,chapter:item.chapter,nodes:[],options:{...item.options,pagination_only:true}});
                const started=performance.now();
                const poll=()=>{
                    try {
                        const events=bridge.drain();
                        for(const event of events){
                            if(event.type==='error')throw Error(event.message);
                            if(event.type==='pagination'){
                                globalThis.__vobrCountResults.push({chapter:item.chapter,starts:event.page_starts});
                                frame.remove();launch();return;
                            }
                        }
                        if(performance.now()-started>30000)throw Error('Chapter pagination timed out');
                        setTimeout(poll,4);
                    } catch(error){globalThis.__vobrCountErrors.push(String(error));frame.remove();}
                };
                poll();
            } catch(error){globalThis.__vobrCountErrors.push(String(error));frame.remove();}
        };
        frame.onerror=()=>globalThis.__vobrCountErrors.push('Chapter resource failed');
        frame.src=item.url;
        document.body.append(frame);
    };
    for(let worker=0;worker<Math.min(3,items.length);worker++)launch();
    return true;
}
