import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const context={};vm.createContext(context);
vm.runInContext(fs.readFileSync(new URL('./src/font_choice.js',import.meta.url),'utf8'),context);
const create=context.__vobrCreateFontChoice;
const style=(family='',weight='400',priority='')=>{
    const values=new Map([['font-family',[family,priority]],['font-weight',[weight,'']]]);
    return {getPropertyValue:key=>values.get(key)?.[0]??'',getPropertyPriority:key=>values.get(key)?.[1]??'',setProperty:(key,value,p='')=>values.set(key,[value,p]),removeProperty:key=>values.delete(key)};
};
const node=(tag,parent,family='JA, serif',weight='400',own='',priority='',attrs={})=>({
    localName:tag,parentElement:parent,family,id:attrs.id??'',style:style(own,weight,priority),
    getAttribute:key=>attrs[key]??null,
    closest(selectors){for(let el=this;el;el=el.parentElement)if(selectors.split(',').includes(el.localName))return el;return null;},
});
const html=node('html',null,'serif'),body=node('body',html,'JA, serif');
const regular=node('p',body),nested=node('span',regular),bold=node('strong',regular,'JA, serif','700');
const own=node('p',body,'"Book, Serif", serif','400','"Book, Serif", serif','important');
const codeParent=node('pre',body),code=node('span',codeParent,'Consolas, monospace');
const math=node('mi',node('math',body),'STIX Two Math');
const svg=node('text',node('svg',body),'Book Diagram');
const pua=node('span',regular),icon=node('span',regular,'Unusual Book Face','400','','',{class:'icon'});
const embedded=node('span',regular,'Embedded Figure');
const embeddedChild=node('b',embedded,'Embedded Figure','700');
const visible=[regular,nested,bold,own,code,math,svg,pua,icon,embeddedChild];
const source=visible.map((element,i)=>{const dom={parentElement:element};return {dom,parts:[{dom}],text:i===7?'\uE123':'漢字ABC'};});
const view={document:{body,fonts:[{family:'Embedded Figure'}]},getComputedStyle:el=>({fontFamily:el.family})};
const choice=create(view,source),before=source.map(x=>x.dom);
choice.apply('Noto Serif CJK JP',false);
assert.equal(regular.style.getPropertyValue('font-family'),'"Noto Serif CJK JP", JA, serif');
assert.equal(nested.style.getPropertyValue('font-family'),'"Noto Serif CJK JP", JA, serif','explicit child alias receives the choice');
assert.equal(bold.style.getPropertyValue('font-weight'),'700','author bold is retained');
assert.equal(own.style.getPropertyValue('font-family'),'"Noto Serif CJK JP", "Book, Serif", serif','quoted family commas remain one family');
for(const element of [code,math,svg,pua,icon,embeddedChild])assert.equal(element.style.getPropertyValue('font-family'),element.family,'code, math and special book fonts are pinned to the original family');
choice.apply('Noto Sans CJK JP',false);
assert.equal(regular.style.getPropertyValue('font-family'),'"Noto Sans CJK JP", JA, serif','changing font never stacks the previous choice');
choice.apply('Noto Sans CJK JP',false);
assert.equal(regular.style.getPropertyValue('font-family'),'"Noto Sans CJK JP", JA, serif','same choice is idempotent');
choice.apply(null,false);
assert.equal(regular.style.getPropertyValue('font-family'),'');
assert.equal(own.style.getPropertyValue('font-family'),'"Book, Serif", serif');
assert.equal(own.style.getPropertyPriority('font-family'),'important','unset restores original inline priority');
choice.apply('Noto Serif CJK JP',true);
assert.equal(regular.style.getPropertyValue('font-family'),'','fixed layout remains authored');
choice.apply('Noto Serif CJK JP',false);
choice.apply('Noto Sans CJK JP',true);
assert.equal(regular.style.getPropertyValue('font-family'),'','switching an active override to fixed layout restores the author');
assert.equal(own.style.getPropertyValue('font-family'),'"Book, Serif", serif');
assert.deepEqual(source.map(x=>x.dom),before,'font changes keep original source nodes');
assert.equal(bold.style.getPropertyValue('font-weight'),'700');
create({},[]).apply('Anything',false);
console.log('font choice: child aliases, fallback families, bold, code/math/SVG, PUA/embedded glyphs, reconfigure and restore passed');
