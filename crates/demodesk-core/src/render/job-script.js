// ClipForge private job bridge. HLAE v2.192.7 public mirv-script API (advancedfx MIT).
// No netcon listener. The game connects out to an authenticated loopback-only job server.
(() => {
  const job = __CLIPFORGE_JOB__;
  const id = "clipforge-job";
  let ws, sending = Promise.resolve(), seeking = null, lastReport = 0, takeFolder = null, frame = 0, lastView = null;
  function emit(detail) {
    if (!ws) return;
    detail.tick = mirv.isPlayingDemo() ? mirv.getDemoTick() : null;
    sending = sending.then(() => ws.out.send(JSON.stringify(detail)))
      .catch(error => mirv.warning("ClipForge telemetry: " + String(error)));
  }
  function pawnFor(entity) {
    if (!entity) return null;
    if (entity.isPlayerPawn()) return entity;
    const handle = entity.getPlayerPawnHandle();
    return mirv.isHandleValid(handle) ? mirv.getEntityFromIndex(mirv.getHandleEntryIndex(handle)) : null;
  }
  function observed() {
    const local = pawnFor(mirv.getEntityFromSplitScreenPlayer(0));
    if (!local) return {steamId:null,alive:null,observerMode:null,source:"unavailable"};
    const handle = local.getObserverTargetHandle();
    const view = mirv.isHandleValid(handle) ? mirv.getEntityFromIndex(mirv.getHandleEntryIndex(handle)) : local;
    if (!view) return {steamId:null,alive:null,observerMode:local.getObserverMode(),source:"unavailable"};
    const controllerHandle = view.getPlayerControllerHandle();
    const controller = mirv.isHandleValid(controllerHandle) ? mirv.getEntityFromIndex(mirv.getHandleEntryIndex(controllerHandle)) : null;
    const steamId = controller ? String(controller.getSteamId()) : null;
    return {steamId:steamId === "0" ? null : steamId, alive:view.getHealth() > 0,
      observerMode:local.getObserverMode(), pawnHandle:handle, source:"source2_observer_handle"};
  }
  function expectedTarget(tick) {
    let target = null;
    for (const choice of job.targets) if (tick >= choice.tick) target = choice.steamId;
    return target;
  }
  mirv.events.clientFrameStageNotify.on(id, event => {
    // Match advancedfx's mirv_script_view: update the camera after stage 5 only when it differs.
    if (event.isBefore || event.curStage !== 5) return;
    const tick = mirv.isPlayingDemo() ? mirv.getDemoTick() : -1;
    const target = expectedTarget(tick);
    if (target && !seeking) {
      for (let index = 1; index <= 64; index++) {
        const entity = mirv.getEntityFromIndex(index);
        if (entity && entity.isPlayerController() && String(entity.getSteamId()) === target) {
          const current=observed();
          if (current.steamId !== target) mirv.exec("spec_mode 1; spec_player " + index);
          else if (current.observerMode !== 2) mirv.exec("spec_mode 1");
          break;
        }
      }
    }
  });
  // HLAE dispatches afterCommands synchronously at the end of this actual capture stream's
  // render pass (CStream::EngineThread_EndFrame). Count capture passes, never generic client frames.
  const captureFrame = new AdvancedfxConCommand(() => {
    if (takeFolder) {
      const tick=mirv.getDemoTick();
      const target=expectedTarget(tick);
      emit({type:"frame", takeFolder, frameIndex:frame, ptsSeconds:frame/job.fps,
        demoTick:tick, demoTime:mirv.getDemoTime(), targetSteamId:target, ...observed(), view:lastView});
      frame++;
    }
  });
  captureFrame.register("clipforge_capture_frame", "Private actual capture-pass boundary");
  const echo = new AdvancedfxConCommand(args => {
    const words = [];
    for (let i = 1; i < args.argC(); i++) words.push(args.argV(i));
    emit({line:words.join(" ")});
  });
  echo.register("clipforge_echo", "Private recording progress marker");
  const seek = new AdvancedfxConCommand(args => {
    const tick = Number(args.argV(1));
    const schedule = job.seekSchedules[String(tick)];
    if (!Number.isInteger(tick) || !schedule) {
      emit({line:"Unknown command: invalid ClipForge seek"});
      return;
    }
    // A seek crosses mirv_cmd intervals. Always disarm, seek, and only then arm future commands.
    mirv.exec("demo_pause; mirv_cmd clear");
    seeking = {tick,schedule,since:Date.now()};
    mirv.exec("demo_gototick " + tick);
  });
  seek.register("clipforge_seek", "Private disarmed demo seek");
  globalThis.clipforgeJobCommands = [echo,seek,captureFrame]; // Keep registered commands alive.
  mirv.events.cViewRenderSetupView.on(id, event => {
    lastView = {width:event.width,height:event.height,x:event.gameView.x,y:event.gameView.y,z:event.gameView.z,fov:event.gameView.fov};
    const tick = mirv.isPlayingDemo() ? mirv.getDemoTick() : -1;
    if (seeking && tick >= seeking.tick - 1 && tick <= seeking.tick + 4 && Date.now() - seeking.since > 100) {
      const schedule = seeking.schedule;
      seeking = null;
      mirv.exec('mirv_cmd load "' + schedule + '"; demo_resume');
      emit({line:"[clipforge] seek completed", width:event.width, height:event.height});
    }
    if (Date.now() - lastReport > 1000) {
      lastReport = Date.now();
      emit({line:"[clipforge] frame", width:event.width, height:event.height});
    }
  });
  mirv.events.recordStart.on(id, event => {
    takeFolder=event.takeFolder; frame=0;
    emit({type:"recordStart", line:"[clipforge] native record start", takeFolder,fps:job.fps,targetSteamId:expectedTarget(mirv.getDemoTick())});
  });
  mirv.events.recordEnd.on(id, () => {
    emit({type:"recordEnd", line:"[clipforge] native record end", takeFolder,frameCount:frame});
    takeFolder=null;
  });
  mirv.connect_async(job.url).then(async connection => {
    ws = connection;
    globalThis.clipforgeJobConnection = connection; // Root both native streams while async reads are pending.
    emit({line:"clipforge_ready"});
    while (true) {
      const message = await ws.in.next();
      if (message === null) break;
      if (typeof message !== "string") continue;
      const request = JSON.parse(message);
      if (typeof request.command !== "string") continue;
      mirv.exec(request.command.replace(/\becho /g, "clipforge_echo "));
    }
  }).catch(error => mirv.warning("ClipForge private bridge: " + String(error)));
})();
