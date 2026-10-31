// Same-origin/CSP fixture runner. File URLs have opaque origins in Chromium,
// so the iframe source-identity case must use this runner instead of file://.
// No author JavaScript, external assets, or security switches are enabled.
import {createServer} from 'node:http';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {once} from 'node:events';
import {spawn} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const argumentsList=process.argv.slice(2);
const argument=name=>{
    const index=argumentsList.indexOf(name);
    if(index<0||!argumentsList[index+1])throw Error(`Required: ${name}`);
    return path.resolve(argumentsList[index+1]);
};
const bundle=argument('--bundle'),work=argument('--work');
const suiteIndex=argumentsList.indexOf('--suite'),suite=suiteIndex<0?'general':argumentsList[suiteIndex+1];
if(!['general','images','fixed'].includes(suite))throw Error('Suite must be general, images, or fixed');
await mkdir(work,{recursive:true});
const testRoot=path.dirname(fileURLToPath(import.meta.url));
const server=createServer(async(request,response)=>{
    const route=new URL(request.url,'http://127.0.0.1').pathname;
    if(suite==='images'&&['/glyph.svg','/colored.svg','/gradient.svg'].includes(route)){
        const body=route==='/glyph.svg'?'<rect width="16" height="16" fill="white"/><rect x="4" y="4" width="8" height="8" fill="black"/>':route==='/colored.svg'?'<rect width="16" height="16" fill="#d02020"/>':'<defs><linearGradient id="g"><stop stop-color="black"/><stop offset="1" stop-color="white"/></linearGradient></defs><rect width="16" height="16" fill="url(#g)"/>';
        response.writeHead(200,{'Content-Type':'image/svg+xml','Cache-Control':'no-store'});response.end(`<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">${body}</svg>`);return;
    }
    if(route!=='/source.xhtml'){
        response.writeHead(404);response.end();return;
    }
    try{
        let bytes=await readFile(path.join(work,'source.xhtml'));
        if(suite==='fixed')bytes=Buffer.from('<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>Fixed original fixture</title><meta name="viewport" content="width=320,height=480"/><style>html,body{margin:0;padding:0;width:320px;height:480px;overflow:hidden}svg{display:block;width:320px;height:480px}</style></head><body><svg xmlns="http://www.w3.org/2000/svg" width="320" height="480" viewBox="0 0 320 480"><rect id="art" width="320" height="480" fill="#0f766e"/><a href="#target"><rect x="64" y="100" width="192" height="100" fill="white"/><text x="100" y="160" font-size="36" fill="black">LINK</text></a><text id="target" x="100" y="440" font-size="32" fill="white">PAGE 2</text></svg></body></html>');
        if(suite==='images')bytes=Buffer.from(bytes.toString('utf8').replace('</h1>','</h1><p>本文<img id="glyph" class="gaiji" src="glyph.svg" style="width:1em;height:1em;background-color:white"/>続き<img id="colored" class="gaiji" src="colored.svg" style="width:1em;height:1em"/><img id="gradient" src="gradient.svg" style="width:1em;height:1em"/>終わり</p>'));
        response.writeHead(200,{
            'Content-Type':'application/xhtml+xml; charset=utf-8',
            'Content-Length':bytes.length,
            'Cache-Control':'no-store',
            'Content-Security-Policy':"default-src 'self' data:; script-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; frame-src 'self'; connect-src 'none'; base-uri 'none'",
        });
        response.end(bytes);
    }catch{
        response.writeHead(404);response.end();
    }
});
server.listen(0,'127.0.0.1');await once(server,'listening');
const script=path.join(work,'run-chromium-http.ps1');
const input=(await readFile(path.join(testRoot,'run-chromium-regression.ps1'),'utf8'))
    .replace('$testRoot=$PSScriptRoot',`$testRoot='${testRoot.replaceAll("'","''")}'`)
    .replace("$start.Environment['WELD_URL']=[Uri]::new($fixture).AbsoluteUri","$start.Environment['WELD_URL']=$env:VOBR_NAV_FIXTURE_URL");
await writeFile(script,input,'utf8');
let exitCode=1;
try{
    const child=spawn('pwsh',['-NoProfile','-File',script,'-Bundle',bundle,'-Work',work,'-Suite',suite],{
        env:{...process.env,VOBR_NAV_FIXTURE_URL:`http://127.0.0.1:${server.address().port}/source.xhtml`},
        windowsHide:true,stdio:['ignore','pipe','pipe'],
    });
    child.stdout.pipe(process.stdout);child.stderr.pipe(process.stderr);
    const [code]=await once(child,'exit');exitCode=code??1;
}finally{
    server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
}
process.exitCode=exitCode;
