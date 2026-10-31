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
        try{
            const bytes=await readFile(path.join(testRoot,'fixtures',path.basename(route)));
            response.writeHead(200,{'Content-Type':'image/svg+xml','Content-Length':bytes.length,'Cache-Control':'no-store'});response.end(bytes);
        }catch{
            response.writeHead(404);response.end();
        }
        return;
    }
    if(route!=='/source.xhtml'){
        response.writeHead(404);response.end();return;
    }
    try{
        const bytes=await readFile(path.join(work,'source.xhtml'));
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
