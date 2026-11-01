import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';

const source=await readFile(new URL('../../reader-app/src/cef_counter.js',import.meta.url),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
const item=(chapter,flow='reflowable')=>({chapter,url:`vobr://counter/${chapter}.xhtml`,width:600,height:800,options:{flow}});

function fixture(){
    const frames=[],commands=[],timers=[];
    let active=0,peak=0,now=0;
    const context={
        document:{
            createElement(name){
                assert.equal(name,'iframe');
                const frame={style:{},connected:false,loaded:false,removals:0,events:[],
                    remove(){this.removals++;assert.equal(this.connected,true);this.connected=false;active--;}};
                frame.contentWindow={frame};frames.push(frame);return frame;
            },
            body:{append(frame){assert.equal(frame.connected,false);frame.connected=true;active++;peak=Math.max(peak,active);}},
        },
        performance:{now:()=>now},
        setTimeout(callback,delay){assert.equal(delay,4);timers.push(callback);},
        __vobrInstallNavigator({frame}){
            return {
                dispatch(command){frame.command=command;commands.push(command);},
                drain(){return frame.events.splice(0);},
            };
        },
    };
    vm.createContext(context);vm.runInContext(source,context);
    const load=(frame,events)=>{
        assert.equal(frame.connected,true);assert.equal(frame.loaded,false);frame.loaded=true;
        frame.events.push(...events);frame.onload();
    };
    const finishAll=()=>{
        for(let frame;(frame=frames.find(value=>value.connected&&!value.loaded));){
            load(frame,[{type:'pagination',page_starts:[null]}]);
        }
    };
    return {
        context,frames,commands,timers,load,finishAll,
        start(items,limit){return context.countChapters(items,limit);},
        enqueue(items){return context.__vobrEnqueueCountItems(items);},
        advance(milliseconds){now+=milliseconds;},
        get active(){return active;},get peak(){return peak;},
        get results(){return plain(context.__vobrCountResults);},
        get errors(){return Array.from(context.__vobrCountErrors);},
    };
}

test('two active iframe slots replenish as each measured chapter completes',()=>{
    const h=fixture(),items=Array.from({length:6},(_,index)=>item(index));
    assert.equal(h.start(items,2),true);assert.equal(h.frames.length,2);assert.equal(h.active,2);
    h.load(h.frames[0],[{type:'pagination',page_starts:[{chapter:0,node:5,offset:0},null]}]);
    assert.equal(h.frames[0].removals,1);assert.equal(h.frames.length,3);assert.equal(h.active,2);
    assert.deepEqual(h.results,[{chapter:0,starts:[{chapter:0,node:5,offset:0},null]}]);
    h.load(h.frames[1],[{type:'pagination',page_starts:[null]}]);
    assert.equal(h.frames.length,4);assert.equal(h.active,2);
    h.finishAll();
    assert.equal(h.peak,2);assert.equal(h.active,0);assert.equal(h.frames.length,6);
    assert.deepEqual(h.results.map(value=>value.chapter).sort((a,b)=>a-b),[0,1,2,3,4,5]);
    assert.ok(h.frames.every(frame=>frame.removals===1));
    assert.ok(h.commands.every(command=>command.type==='configure'&&command.options.pagination_only===true));
    assert.ok(items.every(value=>value.options.pagination_only===undefined));
    assert.deepEqual(h.errors,[]);
});

test('streaming refills deduplicate chapters and fixed pages consume no iframe slot',()=>{
    const h=fixture();h.start([item(0,'fixed')],2);
    assert.equal(h.frames.length,0);assert.deepEqual(h.results,[{chapter:0,starts:[null]}]);
    h.enqueue([item(0,'fixed'),item(1),item(2),item(3)]);
    assert.equal(h.active,2);
    h.enqueue([item(1),item(4,'fixed'),item(3),item(5)]);
    assert.equal(h.frames.length,2);h.finishAll();
    h.enqueue([item(5),item(4,'fixed'),item(6,'fixed')]);
    assert.equal(h.active,0);assert.equal(h.peak,2);assert.equal(h.frames.length,4);
    assert.deepEqual(h.commands.map(command=>command.chapter).sort((a,b)=>a-b),[1,2,3,5]);
    assert.deepEqual(h.results.map(value=>value.chapter).sort((a,b)=>a-b),[0,1,2,3,4,5,6]);
    assert.deepEqual(h.errors,[]);
});

test('resource failure releases one slot and an already scheduled poll becomes inert',()=>{
    const h=fixture();h.start([item(0),item(1),item(2),item(3)],2);
    const failed=h.frames[0];h.load(failed,[]);
    assert.equal(h.timers.length,1);const stalePoll=h.timers.shift();
    failed.onerror();
    assert.equal(failed.removals,1);assert.equal(h.active,2);assert.equal(h.frames.length,3);
    h.advance(31000);stalePoll();
    assert.equal(failed.removals,1);assert.equal(h.active,2);assert.equal(h.timers.length,0);
    h.finishAll();
    assert.equal(h.active,0);assert.equal(h.peak,2);
    assert.deepEqual(h.results.map(value=>value.chapter).sort((a,b)=>a-b),[1,2,3]);
    assert.deepEqual(h.errors,['Chapter resource failed']);
    assert.ok(h.frames.every(frame=>frame.removals===1));
});

test('pagination timeout removes the stalled frame and launches its queued successor',()=>{
    const h=fixture();h.start([item(0),item(1)],1);
    h.load(h.frames[0],[]);h.advance(30001);h.timers.shift()();
    assert.equal(h.frames[0].removals,1);assert.equal(h.frames.length,2);assert.equal(h.active,1);
    h.finishAll();
    assert.equal(h.active,0);assert.equal(h.peak,1);
    assert.deepEqual(h.results,[{chapter:1,starts:[null]}]);
    assert.equal(h.errors.length,1);assert.match(h.errors[0],/Chapter pagination timed out/);
});

test('default remains three slots and invalid limits leave existing results intact',()=>{
    const h=fixture();h.start([item(0),item(1),item(2),item(3)]);
    assert.equal(h.active,3);h.finishAll();assert.equal(h.peak,3);
    const previous=h.results;
    for(const limit of [0,-1,4,1.5,'2',null,NaN,Infinity]){
        assert.throws(()=>h.start([],limit),/Invalid chapter counter active limit/);
        assert.deepEqual(h.results,previous);
    }
    assert.deepEqual(h.errors,[]);
});
