// Reader font choice changes family only. Keep author weights and source nodes.
(() => {
    const families = value => {
        const out=[];let start=0,quote='',escaped=false;
        for(let i=0;i<value.length;i++){
            const ch=value[i];
            if(escaped){escaped=false;continue;}
            if(ch==='\\'){escaped=true;continue;}
            if(quote){if(ch===quote)quote='';continue;}
            if(ch==='"'||ch==="'"){quote=ch;continue;}
            if(ch===','){out.push(value.slice(start,i).trim());start=i+1;}
        }
        out.push(value.slice(start).trim());return out.filter(Boolean);
    };
    const name = value => value.trim().replace(/^(["'])(.*)\1$/s,'$2').replace(/\\([\\"'])/g,'$1').toLowerCase();
    const chosenFamily = (selected, original) => [JSON.stringify(selected),...families(original).filter(f=>name(f)!==selected.toLowerCase())].join(', ');
    const privateUse = /[\uE000-\uF8FF\u{F0000}-\u{FFFFD}\u{100000}-\u{10FFFD}]/u;
    const specialName = /(?:^|[-_\s])(?:gaiji\d*|glyph\d*|icons?|symbols?|dingbats?|formula|equation|math)(?:$|[-_\s])|外字|数式/i;
    const specialFamily = /(?:symbol|dingbat|wingding|webding|icon|math|gaiji|外字)/i;
    const preserve = 'pre,code,kbd,samp,math,svg,script,style,head,input,textarea,select,option,button';
    const create = (view, nodes) => {
        const doc=view.document;
        if(!doc)return {apply(){}};
        // This runs once, before the reader adds its own @font-face rules.
        const authorFaces=new Set(Array.from(doc.fonts??[]).map(face=>name(face.family)));
        const styles=new Map(),computed=el=>{
            let value=styles.get(el);
            if(!value){value=view.getComputedStyle(el).fontFamily;styles.set(el,value);}
            return value;
        };
        const parents=new Map();
        for(const node of nodes){
            if(!node.parts||!node.text.trim())continue;
            const el=node.dom.parentElement;
            if(el)parents.set(el,(parents.get(el)??'')+node.text);
        }
        const plans=[];
        for(const [element,text] of parents){
            if(element.closest('script,style,head'))continue;
            const original=computed(element);
            let protectedFont=!!element.closest(preserve)||privateUse.test(text)||families(original).some(f=>name(f)==='monospace'||specialFamily.test(name(f)));
            for(let el=element;el&&el!==doc.body&&!protectedFont;el=el.parentElement){
                if(specialName.test(`${el.id??''} ${el.getAttribute('class')??''}`)){protectedFont=true;break;}
                // An explicit book font on a separate scope can map ordinary
                // ASCII to symbols; preserving only PUA characters is unsafe.
                const family=computed(el),parent=el.parentElement;
                if(parent&&family!==computed(parent)&&families(family).some(f=>authorFaces.has(name(f))))protectedFont=true;
            }
            plans.push({element,original,protectedFont,inline:element.style.getPropertyValue('font-family'),priority:element.style.getPropertyPriority('font-family')});
        }
        let applied=false;
        return {
            apply(selected,fixed,onWrite=()=>{}){
                if(applied)for(const plan of plans){
                    if(plan.inline)plan.element.style.setProperty('font-family',plan.inline,plan.priority);
                    else plan.element.style.removeProperty('font-family');
                    onWrite(plan.element);
                }
                applied=false;
                if(fixed||!selected)return;
                for(const plan of plans){
                    plan.element.style.setProperty('font-family',plan.protectedFont?plan.original:chosenFamily(selected,plan.original),'important');
                    onWrite(plan.element);
                }
                applied=true;
            },
        };
    };
    Object.defineProperty(globalThis,'__vobrCreateFontChoice',{value:create,configurable:true});
})();
