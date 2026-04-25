//! Symbol-level chunks for ArkTS (`.ets`) using the [tree-sitter-arkts](https://github.com/Million-mo/tree-sitter-arkts) grammar.

use std::path::Path;

use async_trait::async_trait;
use parking_lot::Mutex;
use tree_sitter::Parser;

use crate::common::FileService;
use crate::common::data::Chunk;
use crate::document_chunker::chunker::Chunker;
use crate::document_chunker::symbol::{SymbolKind, SymbolPipeline};
use crate::language::language::Language;

pub struct ArkTsChunker {
    parser: Mutex<Parser>,
    file_service: FileService,
    pipeline: SymbolPipeline,
}

impl ArkTsChunker {
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_arkts::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("tree-sitter ArkTS grammar: {e}"))?;
        Ok(Self {
            parser: Mutex::new(parser),
            file_service: FileService::new(),
            pipeline: SymbolPipeline::new(Language::Arkts.id()),
        })
    }
}

#[async_trait]
impl Chunker for ArkTsChunker {
    async fn split(&self, path: &Path, relative_path: &str) -> anyhow::Result<Vec<Chunk>> {
        let source = self.file_service.read_file_to_string(path).await?;

        let tree = {
            let mut parser = self.parser.lock();
            parser
                .parse(&source, None)
                .ok_or_else(|| anyhow::anyhow!("tree-sitter parse returned None"))?
        };

        Ok(self.pipeline.split_file_to_chunks(
            &tree,
            &source,
            relative_path,
            SymbolKind::from_node_kind,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::document_chunker::chunker::Chunker;

    fn temp_ets_file(name: &str, source: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("arkts_chunker_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(name);
        fs::write(&path, source).expect("write");
        (path, dir)
    }

    fn first_line(s: &str) -> &str {
        s.lines().next().unwrap_or("")
    }

    #[tokio::test]
    async fn split_parses_common_symbol_kinds() {
        let src = r#"import { common, wantAgent } from '@kit.AbilityKit';
import { avSession } from '@kit.AVSessionKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { BackgroundTaskManager, Logger } from '@ohos/utils';
import { AudioRenderService } from './AudioRenderService';
import { SpeechPlayerService } from './SpeechPlayerService';

const TAG = '[AVSessionService]';
const TEXT_TO_AUDIO_LOADING_TIME = 1000;

export enum AudioPlayerStatus {
  LOADING = 'loading',
  IDLE = 'idle',
  PLAYING = 'playing',
  PAUSED = 'paused',
}

export class AudioPlayerService {
  private context: common.UIAbilityContext | undefined = AppStorage.get('uiAbilityContext');
  private speechPlayerService: SpeechPlayerService = SpeechPlayerService.getInstance();
  private audioRenderService: AudioRenderService = AudioRenderService.getInstance();
  private session: avSession.AVSession | undefined = undefined;
  private static instance: AudioPlayerService | null;

  private constructor() {
    this.initAudioPlayerService();
  }

  public static getInstance(): AudioPlayerService {
    if (!AudioPlayerService.instance) {
      AudioPlayerService.instance = new AudioPlayerService();
    }
    return AudioPlayerService.instance;
  }

  setSessionPlayState(state: avSession.PlaybackState) {
    this.session?.setAVPlaybackState({ state });
  }

  private async initAudioPlayerService() {
    await this.speechPlayerService.createTextToSpeechEngine().then(() => {
      if (this.context) {
        this.audioRenderService.initAudioRenderInit();
        this.createSession();
        BackgroundTaskManager.startContinuousTask(this.context);
        AppStorage.setOrCreate<AudioPlayerStatus>('audioPlayerStatus', AudioPlayerStatus.IDLE);
      }
    })
  }

  public createSession() {
    // Access the Broadcast Control Center.
    avSession.createAVSession(this.context, 'SPEECH_AUDIO_SESSION', 'audio').then(async (avSession) => {
      this.session = avSession;
      Logger.info(TAG, 'Succeeded in create avSession.');
      await this.setAVMetadata();
      const wantAgentInfo: wantAgent.WantAgentInfo = {
        wants: [
          {
            bundleName: this.context?.abilityInfo.bundleName,
            abilityName: this.context?.abilityInfo.name
          }
        ],
        operationType: wantAgent.OperationType.START_ABILITIES,
        requestCode: 0,
        wantAgentFlags: [wantAgent.WantAgentFlags.UPDATE_PRESENT_FLAG]
      }
      wantAgent.getWantAgent(wantAgentInfo).then((agent) => {
        this.session?.setLaunchAbility(agent);
      })
      this.setListenerForMesFromController();
      this.session.activate();
    });
  }


  public static destroy() {
    AudioPlayerService.getInstance().releaseAudioPlayer();
    AudioPlayerService.instance = null;
  }
}
"#;
        let (path, dir) = temp_ets_file("sample.ets", src);
        let chunker = ArkTsChunker::new().expect("ArkTsChunker::new");
        let chunks = chunker.split(&path, "sample.ets").await.expect("split");
        assert!(!chunks.is_empty());

        let chunks_gt = vec![
            "AudioPlayerStatus",
            "AudioPlayerService",
            "getInstance",
            "setSessionPlayState",
            "initAudioPlayerService",
            "createSession",
            "destroy",
        ];
        for (chunk, chunk_gt) in chunks.iter().zip(chunks_gt.iter()) {
            assert_eq!(chunk.embedding_content, *chunk_gt);
        }

        fs::remove_dir_all(&dir).ok();
    }
}
