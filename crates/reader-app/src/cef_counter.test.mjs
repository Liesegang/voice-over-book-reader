import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source=fs.readFileSync(new URL('./cef_counter.js',import.meta.url),'utf8');
function fixture(items,{autoDrain=true}={}){
    const pending=[],created=[],commands=[];
    let active=0,peak=0;
    const context={
        document:{
            createElement(name){
                assert.equal(name,'iframe');
                const frame={style:{},remove(){active--;}};
                created.push(frame);
                return frame;
            },
            body:{append(frame){pending.push(frame);active++;peak=Math.max(peak,active);}},
        },
        performance:{now:()=>0},
        setTimeout(){throw Error('A ready fixture must not need timer polling');},
        __vobrInstallNavigator(){
            let command;
            return {
                dispatch(value){command=value;commands.push(value);},
                drain(){return [{type:'pagination',page_starts:[{chapter:command.chapter,node:5,offset:0},null]}];},
            };
        },
    };
    vm.createContext(context);
    vm.runInContext(source,context);
    context.countChapters(items);
    const initial=created.length;
    const drain=()=>{while(pending.length)pending.shift().onload();};
    if(autoDrain)drain();
    return {created,commands,initial,get peak(){return peak;},get active(){return active;},
        enqueue(items){context.__vobrEnqueueCountItems(items);},drain,
        get results(){return JSON.parse(JSON.stringify(context.__vobrCountResults));},
        get errors(){return Array.from(context.__vobrCountErrors);}};
}

const fixed=Array.from({length:5000},(_,chapter)=>({chapter,options:{flow:'fixed'}}));
const fast=fixture(fixed);
assert.equal(fast.created.length,0,'declared fixed pages never create image-loading iframes');
assert.equal(fast.commands.length,0,'fixed page counts need no navigator/font/image work');
assert.equal(fast.results.length,fixed.length,'a long fixed book does not recurse per page');
assert.ok(fast.results.every((item,index)=>item.chapter===index&&item.starts.length===1&&item.starts[0]===null));
assert.deepEqual(fast.errors,[]);

const mixed=Array.from({length:15},(_,chapter)=>({chapter,url:`vobr://test/${chapter}.xhtml`,width:600,height:800,options:{flow:chapter%3===0?'fixed':'reflowable'}}));
const counted=fixture(mixed);
assert.equal(counted.created.length,10,'only reflowable chapters need a DOM measurement');
assert.equal(counted.initial,3,'image-heavy measurement remains bounded to three iframes');
assert.equal(counted.peak,3);
assert.equal(counted.active,0);
assert.deepEqual(counted.results.map(item=>item.chapter).sort((a,b)=>a-b),mixed.map(item=>item.chapter));
for(const command of counted.commands){
    assert.equal(command.options.pagination_only,true,'counter requests dimension-ready layout without display rasterization');
    assert.equal(command.options.flow,'reflowable');
}
assert.ok(mixed.every(item=>item.options.pagination_only===undefined),'counter flags do not mutate display options');
assert.deepEqual(counted.errors,[]);

// Native discovery can provide chapters after the initial queue is empty.
// Earlier results must survive every refill, and new arrivals cannot bypass
// the three-worker bound or start the same chapter a second time.
const streaming=fixture([],{autoDrain:false});
assert.equal(streaming.created.length,0);
streaming.enqueue(mixed.slice(0,2));
assert.equal(streaming.results.length,1,'known image metadata is immediately visible');
assert.equal(streaming.active,1);
streaming.enqueue(mixed.slice(1,8));
assert.equal(streaming.active,3);
streaming.drain();
assert.equal(streaming.active,0);
const earlier=streaming.results.length;
streaming.enqueue(mixed.slice(8));
streaming.drain();
assert.equal(streaming.results.length,mixed.length);
assert.ok(earlier>0&&earlier<mixed.length,'partial progress precedes later discovery');
assert.equal(streaming.created.length,10,'duplicate plan arrivals never repeat DOM work');
assert.equal(streaming.peak,3);
assert.deepEqual(streaming.results.map(item=>item.chapter).sort((a,b)=>a-b),mixed.map(item=>item.chapter));
assert.deepEqual(streaming.errors,[]);
console.log('chapter counter: fixed metadata, streaming discovery and bounded measurement passed');
