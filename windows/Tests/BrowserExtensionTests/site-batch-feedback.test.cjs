const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const script=['huggingface.js','batch-ui.js'].map(name=>fs.readFileSync('browser-extension/'+name,'utf8')).join('\n');
const items=['first','second','third'].map((name,index)=>({url:'https://pixabay.com/music/'+name+'-'+(123+index)+'/',title:name,resolve:true}));
const keys=items.map(item=>item.url);
let state=null,permissionRequests=0;
const calls=[];
function open(){
  const element=()=>({style:{},children:[],append(...elements){this.children.push(...elements);},
    replaceChildren(){this.children=[];}});
  const nodes={},listeners=[];
  for(const id of ['batch','batch-actions','results','select-all','download-selected','hint','site-batch-format-row','site-batch-format','site-batch-progress','site-batch-controls','site-batch-verify','site-batch-cancel','site-batch-select-label'])nodes[id]=element();
  nodes['site-batch-format'].value='mp3';
  const browser={permissions:{request:async()=>{permissionRequests++;return true;}},
    tabs:{query:async()=>[{id:7,url:'https://pixabay.com/music/'}],connect:(tabId,{name,frameId})=>{calls.push({type:'port',tabId,name,frameId});}},
    runtime:{
      onMessage:{addListener:listener=>listeners.push(listener)},
      async sendMessage(msg){
        calls.push(msg);
        if(msg.type==='getSiteBatchStatus')return {batch:state,tabId:7};
        if(msg.type==='scanLinks')return {items,checked:true};
        if(msg.type==='downloadLinks'){
          assert.equal(nodes['download-selected'].disabled,true);
          push({id:'current',items,total:3,done:0,ready:[],delivered:[],unchecked:[],running:true});
          return {ok:true,queued:3};
        }
        if(msg.type==='setSiteBatchSelection'){
          state={...state,unchecked:[...msg.unchecked]};return {ok:true};
        }
        if(msg.type==='downloadCollectedSiteBatch'){
          assert.equal(nodes['download-selected'].disabled,true);
          push({...state,delivered:[...state.delivered,...msg.keys]});
          return {ok:true,keys:msg.keys};
        }
        return {ok:true};
      }
    }
  };
  function push(value){state=value;listeners[0]({type:'siteBatchProgressUI',tabId:7,batch:value});}
  const document={getElementById:id=>nodes[id],createElement:element,body:{classList:{add(){}}}};
  vm.runInNewContext(script,{document,chrome:browser,URL,ShSources:{label:()=> 'Pixabay'}});
  return {nodes,push,rows:()=>nodes.results.children.map(row=>row.children[0])};
}
(async()=>{
  let view=open();await new Promise(setImmediate);
  await view.nodes.batch.onclick();assert.match(view.nodes['download-selected'].textContent,/下载所选/);
  await view.nodes['download-selected'].onclick();
  assert(view.rows().every(row=>row.checked && row.disabled),'待解析行保留勾选但无法提前下载');
  assert.equal(permissionRequests,1);
  assert.equal(view.nodes['site-batch-progress'].hidden,false);
  view.push({...state,done:1,ready:[keys[0]],canView:true});
  assert.equal(view.rows()[0].disabled,false);
  assert.equal(view.rows()[1].disabled,true);
  assert.equal(view.nodes['site-batch-verify'].hidden,false);
  view.rows()[0].checked=false;view.rows()[0].onchange();

  view=open();await new Promise(setImmediate);
  assert.deepEqual(calls.filter(msg=>msg.type==='port').map(msg=>[msg.tabId,msg.name,msg.frameId]),[[7,'pixabayBatchPopup',0],[7,'pixabayBatchPopup',0]]);
  assert.equal(view.rows().length,3,'重开后恢复整批列表');
  assert.equal(view.rows()[0].checked,false,'恢复用户取消勾选');
  assert.equal(view.nodes['site-batch-controls'].hidden,false);
  view.push({...state,done:2,ready:keys.slice(0,2)});
  assert.equal(view.rows()[0].checked,false,'后续进度不会盖掉手动勾选');
  assert.equal(view.rows()[1].checked,true);
  await view.nodes['download-selected'].onclick();
  assert.deepEqual(Array.from(calls.find(msg=>msg.type==='downloadCollectedSiteBatch').keys),[keys[1]]);
  assert.equal(view.rows()[1].disabled,true);
  assert.equal(state.running,true);
  assert.equal(view.rows()[2].checked,true);
  view.push({...state,running:false,done:3,ready:keys});
  assert.equal(view.rows()[2].disabled,false);
  assert.equal(view.nodes['site-batch-controls'].hidden,true);
  assert.equal(view.nodes['site-batch-progress'].hidden,true);
  console.log('进度中勾选、只下载选中就绪项、重开恢复列表和选择通过');
})().catch(error=>{console.error(error);process.exitCode=1});
