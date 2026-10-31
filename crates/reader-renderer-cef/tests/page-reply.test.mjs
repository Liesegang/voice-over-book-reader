import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';

// Install the production navigator against an empty DOM without configuring it.
// The fence itself receives deterministic frame, dispatch and drain callbacks.
const root={nodeType:1,childNodes:[],getAttribute:()=>null};
const context={
    document:{documentElement:root,body:root,fonts:[],addEventListener(){}},
    performance:{now:()=>0},TextEncoder,setTimeout,clearTimeout,console,
    __vobrCreateFontChoice:()=>({apply(){}}),
};
vm.runInNewContext(await readFile(new URL('../src/navigator.js',import.meta.url),'utf8'),context);
const create=context.__vobrBook.testing.createPageReply;
const plain=value=>JSON.parse(JSON.stringify(value));

function harness(fail=false){
    const state={serial:1,revision:9,ticket:0},queue=[],frames=[];
    let dispatches=0;
    const reply=create({requestAnimationFrame:fn=>frames.push(fn)},command=>{
        dispatches++;state.ticket++;
        queue.push(fail?{type:'error',revision:9,message:'invalid page'}:{type:'position',revision:9,page:command.index});
    },()=>queue.splice(0),()=>({...state}));
    return {state,queue,reply:(index,key='6219260402335481857')=>plain(reply({type:'page',revision:9,index},key)),
        frame(){for(const fn of frames.splice(0))fn();},get dispatches(){return dispatches;},get scheduled(){return frames.length;}};
}

test('acknowledgement waits for two frames while polls never redispatch Page',()=>{
    const h=harness();h.queue.push({type:'selection',revision:9});
    assert.deepEqual(h.reply(2),{pending:true,cancelled:false,events:[]});
    assert.equal(h.reply(2).pending,true);assert.equal(h.dispatches,1);
    h.frame();assert.equal(h.reply(2).pending,true);
    h.queue.push({type:'rects',revision:9});
    h.frame();const result=h.reply(2);
    assert.equal(result.pending,false);assert.equal(result.cancelled,false);
    assert.deepEqual(result.events.map(event=>event.type),['selection','position','rects']);
    assert.equal(h.dispatches,1);
    assert.deepEqual(h.reply(2),{pending:false,cancelled:true,events:[]});
});

test('superseded frame callbacks cannot complete a new request',()=>{
    const h=harness();h.queue.push({type:'selection',revision:9});h.reply(1,'1');
    h.frame();h.reply(2,'2');
    h.frame();assert.equal(h.reply(2,'2').pending,true);
    h.frame();const result=h.reply(2,'2');
    assert.deepEqual(result.events.map(event=>event.type),['selection','position']);
    assert.equal(result.events[1].page,2);assert.equal(h.dispatches,2);
});

test('document or turn invalidation drops stale Position and preserves unrelated events',()=>{
    for(const field of ['serial','revision','ticket']){
        const h=harness();h.queue.push({type:'selection',revision:9});h.reply(1);h.frame();
        h.state[field]++;h.queue.push({type:'invalidated',revision:9,reason:'viewport_changed'});
        const result=h.reply(1);assert.equal(result.pending,false);assert.equal(result.cancelled,true);
        assert.deepEqual(result.events.map(event=>event.type),['selection','invalidated']);
        h.frame();assert.equal(h.reply(1).events.length,0);
    }
});

test('dispatch failures settle immediately without waiting for frames',()=>{
    const h=harness(true);const result=h.reply(7);
    assert.equal(result.pending,false);assert.equal(result.cancelled,false);
    assert.equal(result.events[0].type,'error');assert.equal(h.scheduled,0);
});

test('completion keys remain strings and cannot change the target',()=>{
    const h=harness();h.reply(1,'9007199254740993');
    assert.throws(()=>h.reply(2,'9007199254740993'),/changed target/);
    assert.throws(()=>h.reply(1,9007199254740993),/Invalid Page/);
});
