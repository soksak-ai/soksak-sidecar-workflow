# soksak-sidecar-workflow

soksak의 비공개 네이티브 워크플로 런타임이다. `workflow-doc@0.0.1` 실행, 상주 서비스
구현, provider 프로세스 감독, `soksak-spec-sidecar-workflow` 인터페이스를 소유한다.
이 사이드카가 유일한 Rust 워크플로 런타임이다. workflow 플러그인 저장소는 순수
JS다. plugin manifest(`plugin.json`)와 JS 실행 원장 반쪽(issue lease/receipt/gate/drift)을
소유한다.

## 경계

- 플랫폼 서비스 wire와 하니스는 `Cargo.toml` 및 `validation/spec-validator.json`에
  선언한 공개 `soksak-spec`의 정확한 Git 커밋에서 가져온다.
- 사이드카 고유 payload, 명령, 번들 workflow 문서, 적합성 테스트는 이 저장소가
  소유한다.
- workflow 문서와 기본 저작 참조는 바이너리에 컴파일된다. 실행 위치와 현재
  디렉터리는 런타임 계약이 아니다.
- `--refs <directory>`는 명시적인 개발 override다. 암묵적인 로컬 경로 검색은 없다.
- 실행 stream은 변경하지 않는 일반 파일이다. `latest.json`은 설정된 run 디렉터리
  안의 최신 stream 이름을 담는 일반 JSON 파일이다.

## 인터페이스

`soksak-sidecar-workflow --handshake`는 provider를 시작하지 않고 단위와 도메인
인터페이스를 보고한다. `serve`는 공개 `soksak-spec-service` NDJSON 프로토콜을
시작한다. `exec-one`, `exec-stage`, `synth`, `build-ledger`, `validate-draft`도 같은
구현을 사용하는 결정적 진입점이다.

정확한 계약은 [INTERFACE.md](INTERFACE.md)에 있다.

## 개발

```sh
make test-unit
```

릴리스 workflow는 선언된 데스크톱 5개 target을 빌드하고 `v0.0.1` GitHub Release
asset만 발행한다. crates.io와 npm은 발행 경계가 아니다.
