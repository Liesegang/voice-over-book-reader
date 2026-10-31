import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source=fs.readFileSync(new URL('./cef_counter.js',import.meta.url),'utf8');
function fixture(items){
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
    while(pending.length)pending.shift().onload();
    return {created,commands,initial,peak,active,results:JSON.parse(JSON.stringify(context.__vobrCountResults)),errors:Array.from(context.__vobrCountErrors)};
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
console.log('chapter counter: fixed metadata fast path, mixed exact counts and bounded measurement passed');
