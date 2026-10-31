// Standalone Windows CEF probe. Full test code travels over controlled HTTP,
// never through the native eval/environment string size limit.
import {createServer} from 'node:http';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {once} from 'node:events';
import {spawn} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const args=process.argv.slice(2),argument=name=>{const at=args.indexOf(name);if(at<0||!args[at+1])throw Error('Required '+name);return path.resolve(args[at+1]);};
const bundle=argument('--bundle'),work=argument('--work'),caseAt=args.indexOf('--case'),onlyCase=caseAt<0?null:args[caseAt+1];
await mkdir(work,{recursive:true});
const here=path.dirname(fileURLToPath(import.meta.url)),repo=path.resolve(here,'../../..');
const face="@font-face{font-family:'Noto Serif CJK JP';src:url('/serif.otf') format('opentype');font-display:block;}";
const defaultCss=face+'\n'+await readFile(path.join(repo,'assets/default-reader.css'),'utf8');
const script=`globalThis.__vobrViewportOnlyCase=${JSON.stringify(onlyCase)};globalThis.__vobrViewportDefaultCss=${JSON.stringify(defaultCss)};\n`+await readFile(path.join(here,'../src/font_choice.js'),'utf8')+'\n'+await readFile(path.join(here,'../src/navigator.js'),'utf8')+'\n'+await readFile(path.join(here,'prose-viewport.js'),'utf8');
const font=await readFile(path.join(repo,'assets/fonts/NotoSerifCJKjp-Regular.otf'));
const sentence='窓辺の光が、静かな部屋を照らしていました。「ここから先へ、ゆっくり歩いてみよう。」';
const paragraph='朝の<ruby>光<rt>ひかり</rt></ruby>を眺めながら、川沿いの<ruby>木立<rt>こだち</rt></ruby>を思い出します。'+sentence.repeat(24);
let result=null;
const server=createServer(async(request,response)=>{
    const url=new URL(request.url,'http://127.0.0.1');
    if(url.pathname==='/serif.otf'){response.writeHead(200,{'Content-Type':'font/otf','Content-Length':font.length,'Cache-Control':'max-age=3600'});response.end(font);return;}
    if(url.pathname==='/bootstrap.js'){response.writeHead(200,{'Content-Type':'text/javascript; charset=utf-8','Content-Length':Buffer.byteLength(script)});response.end(script);return;}
    if(url.pathname==='/result'&&request.method==='POST'){
        let body='';for await(const chunk of request)body+=chunk;
        result=JSON.parse(body);await writeFile(path.join(work,'result.json'),JSON.stringify(result,null,2)+'\n');response.writeHead(204);response.end();return;
    }
    if(url.pathname==='/case.xhtml'){
        const mode=url.searchParams.get('mode')==='vertical'?'vertical-rl':'horizontal-tb',code=url.searchParams.get('code')==='1',fixed=url.searchParams.get('fixed')==='1';
        const codeBlock=code?'<h2>短いRustの例</h2><pre><code>fn main() {\n    let message = "こんにちは、読書の時間です";\n    println!("{message}");\n}</code></pre>':'';
        const fixedStyle=fixed?'html,body{width:560px;height:680px;margin:0}body{padding:13px;box-sizing:border-box}':'';
        const html=`<!doctype html><html lang="ja"><head><meta charset="utf-8"><title>Original prose viewport fixture</title><style>body{writing-mode:${mode}}p{margin:0;text-indent:1em}${fixedStyle}</style></head><body><h1>声とページの流れ</h1>${codeBlock}<p>${fixed?paragraph.slice(0,100):paragraph}</p>${fixed?'':`<h2>次の節</h2><p>${paragraph}</p>`}</body></html>`;
        response.writeHead(200,{'Content-Type':'text/html; charset=utf-8','Content-Security-Policy':"default-src 'self';script-src 'none';style-src 'self' 'unsafe-inline';font-src 'self';connect-src 'none'"});response.end(html);return;
    }
    response.writeHead(200,{'Content-Type':'text/html; charset=utf-8','Content-Security-Policy':"default-src 'self';script-src 'self';style-src 'self' 'unsafe-inline';font-src 'self';connect-src 'self';frame-src 'self'"});response.end('<!doctype html><html><head><meta charset="utf-8"><title>Prose viewport regression</title><script src="/bootstrap.js" defer></script></head><body></body></html>');
});
server.listen(0,'127.0.0.1');await once(server,'listening');
const child=spawn(path.join(bundle,'demo-weld-win.exe'),[],{cwd:bundle,windowsHide:true,env:{...process.env,CEF_PATH:bundle,WELD_CACHE_ROOT:path.join(work,'cache'),WELD_TIMEOUT_SECS:'35',WELD_URL:`http://127.0.0.1:${server.address().port}/`,WELD_SCRIPT:"globalThis.__vobrBook ? 'viewport helper installed' : 'viewport helper absent'",WELD_SNAPSHOT:path.join(work,'viewport.png'),RUST_LOG:'warn'},stdio:['ignore','pipe','pipe']});
let log='';child.stdout.on('data',chunk=>log+=chunk.toString());child.stderr.on('data',chunk=>log+=chunk.toString());
const deadline=setTimeout(()=>child.kill(),45000);
const [code]=await once(child,'exit');clearTimeout(deadline);server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
await writeFile(path.join(work,'chromium.log'),log,'utf8');
console.log(JSON.stringify({exitCode:code,report:path.join(work,'result.json'),completed:!!result,fixedUnchanged:result?.fixedUnchanged,cases:result?.cases?.map(test=>({name:test.name,pages:test.pages,viewportClear:test.viewportClear,contentClear:test.contentClear,outsideViewport:test.measured.reduce((n,p)=>n+p.outsideViewport,0),outsideContent:test.measured.reduce((n,p)=>n+p.outsideContent,0)})),error:result?.error??null}));
if(code!==0||!result||result.error||!result.allViewportClear||!result.allContentClear||!result.fixedUnchanged)process.exitCode=1;
