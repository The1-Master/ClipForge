// Share visible-card reads and finish an old cancellation before a replacement read.
export function createMediaBroker<T>(read:(jobId:string,outputKey:string)=>Promise<T>,cancel:(jobId:string,outputKey:string)=>Promise<unknown>) {
  type Record = {promise:Promise<T>;clients:number;pending:boolean;started:boolean;cancelled:boolean;cancelSettled:boolean;cancelAck?:Promise<unknown>};
  const records=new Map<string,Record>();
  return {acquire(jobId:string,outputKey:string){
    const key=jobId+'|'+outputKey;
    const previous=records.get(key);
    let record=previous;
    if(!record||record.cancelled){
      const current:Record={promise:undefined as unknown as Promise<T>,clients:0,pending:true,started:false,cancelled:false,cancelSettled:true};
      const barrier=previous?.cancelled?Promise.allSettled([previous.promise,previous.cancelAck??Promise.resolve()]):Promise.resolve();
      current.promise=barrier.then(()=>{
        if(current.cancelled)throw new Error('Preview request is no longer visible.');
        current.started=true;
        return read(jobId,outputKey);
      });
      record=current;records.set(key,current);
      const settled=()=>{current.pending=false;if(current.clients===0&&current.cancelSettled&&records.get(key)===current)records.delete(key);};
      void current.promise.then(settled,settled);
    }
    record.clients++;
    const current=record;
    let released=false;
    return {promise:current.promise,release(){
      if(released)return;released=true;current.clients--;
      if(current.clients!==0)return;
      if(!current.pending){if(current.cancelSettled&&records.get(key)===current)records.delete(key);return;}
      // StrictMode and same-turn route handoffs can acquire the same request first.
      queueMicrotask(()=>{
        if(current.clients!==0||!current.pending||current.cancelled)return;
        current.cancelled=true;
        if(current.started){
          current.cancelSettled=false;
          current.cancelAck=Promise.resolve().then(()=>cancel(jobId,outputKey)).catch(()=>undefined).finally(()=>{
            current.cancelSettled=true;
            if(current.clients===0&&!current.pending&&records.get(key)===current)records.delete(key);
          });
        }
      });
    }};
  }};
}
