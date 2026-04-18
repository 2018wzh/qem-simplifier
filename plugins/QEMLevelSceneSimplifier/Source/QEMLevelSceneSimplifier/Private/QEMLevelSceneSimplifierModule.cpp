#include "QEMLevelSceneSimplifierModule.h"

#include "AssetToolsModule.h"
#include "Async/Async.h"
#include "Components/SceneComponent.h"
#include "Components/StaticMeshComponent.h"
#include "Editor.h"
#include "Engine/Selection.h"
#include "Engine/StaticMesh.h"
#include "EngineUtils.h"
#include "Framework/Docking/TabManager.h"
#include "HAL/PlatformProcess.h"
#include "HAL/PlatformMisc.h"
#include "Interfaces/IPluginManager.h"
#include "IMeshReductionInterfaces.h"
#include "IMeshReductionManagerModule.h"
#include "LevelEditor.h"
#include "MeshDescription.h"
#include "MeshReductionSettings.h"
#include "Misc/DateTime.h"
#include "Misc/Paths.h"
#include "Misc/PackageName.h"
#include "Modules/ModuleManager.h"
#include "OverlappingCorners.h"
#include "StaticMeshAttributes.h"
#include "StaticMeshOperations.h"
#include "ToolMenus.h"
#include "Widgets/Input/SButton.h"
#include "Widgets/Input/SCheckBox.h"
#include "Widgets/Input/SEditableTextBox.h"
#include "Widgets/Input/SNumericEntryBox.h"
#include "Widgets/Layout/SBox.h"
#include "Widgets/Layout/SSeparator.h"
#include "Widgets/Notifications/SProgressBar.h"
#include "Widgets/SBoxPanel.h"
#include "Widgets/Text/STextBlock.h"
#include "Widgets/Views/STableRow.h"
#include "Widgets/Views/STreeView.h"

#include "qem_simplifier.h"

#define LOCTEXT_NAMESPACE "FQEMLevelSceneSimplifierModule"

DEFINE_LOG_CATEGORY_STATIC(LogQEMLevelSceneSimplifier, Log, All);

struct FLevelSceneMeshData
{
    TWeakObjectPtr<UStaticMesh> StaticMesh;
    FString MeshName;

    TArray<float> Vertices;
    TArray<uint32> Indices;
    TArray<int32> MaterialIds;
};

struct FLevelSceneNodeData
{
    int32 ParentIndex = -1;
    int32 MeshIndex = -1;
    FString NodeKey;
    float WorldMatrix[16] = {0};
    TWeakObjectPtr<UStaticMeshComponent> SourceComponent;
};

struct FLevelSceneCapture
{
    TArray<FLevelSceneMeshData> Meshes;
    TArray<FLevelSceneNodeData> Nodes;
    int32 RootNode = -1;
};

struct FComputedSimplifyPlan
{
    FLevelSceneCapture Capture;
    TArray<QemSceneMeshDecision> Decisions;
    uint32 DecisionCount = 0;
    uint64 SourceTriangleCount = 0;
    uint64 TargetTriangleCount = 0;
    bool bOnlySelectedActors = false;
    float TargetRatio = 1.0f;
    float MinMeshRatio = 0.0f;
    float MaxMeshRatio = 1.0f;
};

namespace
{
    constexpr uint32 QemExpectedAbiVersion = 7;

    using FnQemGetAbiVersion = decltype(&qem_get_abi_version);
    using FnQemSceneGraphComputeDecisions = decltype(&qem_scene_graph_compute_decisions);

    void* GQemRuntimeDllHandle = nullptr;
    bool bQemApiReady = false;
    FString GLoadedQemDllPath;

    FnQemGetAbiVersion GFnQemGetAbiVersion = nullptr;
    FnQemSceneGraphComputeDecisions GFnQemSceneGraphComputeDecisions = nullptr;

    FString GetSystemErrorMessage()
    {
        TCHAR Buffer[512] = {0};
        FPlatformMisc::GetSystemErrorMessage(Buffer, UE_ARRAY_COUNT(Buffer), 0);
        return FString(Buffer);
    }

    template <typename T>
    bool LoadExport(void* DllHandle, const TCHAR* Name, T& OutFn)
    {
        OutFn = reinterpret_cast<T>(FPlatformProcess::GetDllExport(DllHandle, Name));
        return OutFn != nullptr;
    }

    template <typename TLambda>
    bool RunOnGameThreadSync(TLambda&& InLambda)
    {
        if (IsInGameThread())
        {
            InLambda();
            return true;
        }

        FEvent* CompletionEvent = FPlatformProcess::GetSynchEventFromPool(true);
        if (!CompletionEvent)
        {
            return false;
        }

        AsyncTask(ENamedThreads::GameThread, [Task = TUniqueFunction<void()>(Forward<TLambda>(InLambda)), CompletionEvent]() mutable
        {
            Task();
            CompletionEvent->Trigger();
        });

        CompletionEvent->Wait();
        FPlatformProcess::ReturnSynchEventToPool(CompletionEvent);
        return true;
    }

    bool LoadQemRuntimeDll(const FString& OverridePath, FString& OutLoadedPath)
    {
        OutLoadedPath.Empty();

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[DLL] Begin loading qem_simplifier.dll. OverridePath=%s"),
            OverridePath.IsEmpty() ? TEXT("<empty>") : *OverridePath);

        TArray<FString> Candidates;
        if (!OverridePath.IsEmpty())
        {
            Candidates.Add(OverridePath);
        }

        const TSharedPtr<IPlugin> Plugin = IPluginManager::Get().FindPlugin(TEXT("QEMLevelSceneSimplifier"));
        if (!Plugin.IsValid())
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[DLL] Cannot find plugin descriptor: QEMLevelSceneSimplifier"));
            return false;
        }

        const FString BaseDir = Plugin->GetBaseDir();
        const FString PluginBinDir = FPaths::Combine(BaseDir, TEXT("Binaries"), TEXT("Win64"));
        const FString ThirdPartyBinDir = FPaths::Combine(BaseDir, TEXT("Source"), TEXT("ThirdParty"), TEXT("QEMSimplifier"), TEXT("Binary"), TEXT("Win64"));
        Candidates.Add(FPaths::Combine(PluginBinDir, TEXT("qem_simplifier.dll")));
        Candidates.Add(FPaths::Combine(ThirdPartyBinDir, TEXT("qem_simplifier.dll")));

        for (const FString& Candidate : Candidates)
        {
            const FString FullPath = FPaths::ConvertRelativePathToFull(Candidate);
            if (!FPaths::FileExists(FullPath))
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[DLL] Candidate missing: %s"), *FullPath);
                continue;
            }

            UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[DLL] Trying candidate: %s"), *FullPath);

            void* Handle = FPlatformProcess::GetDllHandle(*FullPath);
            if (Handle != nullptr)
            {
                GQemRuntimeDllHandle = Handle;
                OutLoadedPath = FullPath;
                UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[DLL] Loaded successfully: %s"), *FullPath);
                return true;
            }

            UE_LOG(
                LogQEMLevelSceneSimplifier,
                Error,
                TEXT("[DLL] Load failed: %s | SystemError=%s"),
                *FullPath,
                *GetSystemErrorMessage());
        }

        UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[DLL] All candidates failed."));

        return false;
    }

    bool EnsureQemApiLoaded(const FString& OverridePath, FString& OutError)
    {
        OutError.Empty();
        if (bQemApiReady)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Verbose, TEXT("[DLL] API already loaded. Path=%s"), *GLoadedQemDllPath);
            return true;
        }

        FString LoadedPath;
        if (!LoadQemRuntimeDll(OverridePath, LoadedPath))
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[DLL] Failed before export checks."));
            OutError = TEXT("未找到或无法加载 qem_simplifier.dll。请将 DLL 放到插件目录 Source/ThirdParty/QEMSimplifier/Binary/Win64，或在 UI 中指定 DLL 路径。");
            return false;
        }

        const bool bLoadedAbiVersion = LoadExport(GQemRuntimeDllHandle, TEXT("qem_get_abi_version"), GFnQemGetAbiVersion);
        const bool bLoadedSceneDecisions = LoadExport(GQemRuntimeDllHandle, TEXT("qem_scene_graph_compute_decisions"), GFnQemSceneGraphComputeDecisions);
        const bool bAllLoaded = bLoadedAbiVersion && bLoadedSceneDecisions;

        if (!bLoadedAbiVersion)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[DLL] Missing export: qem_get_abi_version (%s)"), *LoadedPath);
        }

        if (!bLoadedSceneDecisions)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[DLL] Missing export: qem_scene_graph_compute_decisions (%s)"), *LoadedPath);
        }

        if (!bAllLoaded)
        {
            if (GQemRuntimeDllHandle != nullptr)
            {
                FPlatformProcess::FreeDllHandle(GQemRuntimeDllHandle);
                GQemRuntimeDllHandle = nullptr;
            }

            GFnQemGetAbiVersion = nullptr;
            GFnQemSceneGraphComputeDecisions = nullptr;

            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[DLL] Export check failed. Unloaded: %s"), *LoadedPath);
            OutError = FString::Printf(TEXT("DLL 已加载但导出符号不完整：%s"), *LoadedPath);
            return false;
        }

        const uint32 AbiVersion = GFnQemGetAbiVersion();
        if (AbiVersion != QemExpectedAbiVersion)
        {
            if (GQemRuntimeDllHandle != nullptr)
            {
                FPlatformProcess::FreeDllHandle(GQemRuntimeDllHandle);
                GQemRuntimeDllHandle = nullptr;
            }

            GFnQemGetAbiVersion = nullptr;
            GFnQemSceneGraphComputeDecisions = nullptr;

            UE_LOG(
                LogQEMLevelSceneSimplifier,
                Error,
                TEXT("[DLL] ABI mismatch. expected=%u, got=%u, path=%s"),
                QemExpectedAbiVersion,
                AbiVersion,
                *LoadedPath);
            OutError = FString::Printf(TEXT("DLL ABI 不匹配：期望=%u，实际=%u（%s）"), QemExpectedAbiVersion, AbiVersion, *LoadedPath);
            return false;
        }

        GLoadedQemDllPath = LoadedPath;
        bQemApiReady = true;
        UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[DLL] API ready. ABI=%u, Path=%s"), AbiVersion, *LoadedPath);
        return true;
    }

    FString MatrixToCompactString(const FMatrix& Matrix)
    {
        return FString::Printf(
            TEXT("[[%.3f %.3f %.3f %.3f] [%.3f %.3f %.3f %.3f] [%.3f %.3f %.3f %.3f] [%.3f %.3f %.3f %.3f]]"),
            Matrix.M[0][0], Matrix.M[0][1], Matrix.M[0][2], Matrix.M[0][3],
            Matrix.M[1][0], Matrix.M[1][1], Matrix.M[1][2], Matrix.M[1][3],
            Matrix.M[2][0], Matrix.M[2][1], Matrix.M[2][2], Matrix.M[2][3],
            Matrix.M[3][0], Matrix.M[3][1], Matrix.M[3][2], Matrix.M[3][3]);
    }

    struct FRunProgressBridge
    {
        TAtomic<float>* Progress = nullptr;
        FCriticalSection* StatusMutex = nullptr;
        FString* StatusText = nullptr;
    };

    struct FStageProgressBridge
    {
        TAtomic<float>* Progress = nullptr;
        FCriticalSection* StatusMutex = nullptr;
        FString* StatusText = nullptr;
        float Start = 0.0f;
        float End = 1.0f;
    };

    void UpdateStageProgress(const FStageProgressBridge* Bridge, float LocalPercent, const FString& NewStatus)
    {
        if (!Bridge || !Bridge->Progress || !Bridge->StatusMutex || !Bridge->StatusText)
        {
            return;
        }

        const float ClampedLocal = FMath::Clamp(LocalPercent, 0.0f, 1.0f);
        const float GlobalPercent = FMath::Lerp(Bridge->Start, Bridge->End, ClampedLocal);
        Bridge->Progress->Store(FMath::Clamp(GlobalPercent, 0.0f, 1.0f));

        FScopeLock Lock(Bridge->StatusMutex);
        *Bridge->StatusText = NewStatus;
    }

    struct FBackupResult
    {
        int32 Attempted = 0;
        int32 Succeeded = 0;
        FString BackupFolder;
    };

    void QemProgressThunk(const QemProgressEvent* Event, void* UserData)
    {
        if (!Event || !UserData)
        {
            return;
        }

        FRunProgressBridge* Bridge = static_cast<FRunProgressBridge*>(UserData);
        if (!Bridge->Progress || !Bridge->StatusMutex || !Bridge->StatusText)
        {
            return;
        }

        const float Pct = FMath::Clamp(Event->percent, 0.0f, 1.0f);
        Bridge->Progress->Store(Pct);

        const FString NewStatus = FString::Printf(
            TEXT("运行中：%.1f%%（mesh %u/%u，src=%u，target=%u，out=%u，status=%d）"),
            Pct * 100.0f,
            Event->mesh_index + 1,
            Event->mesh_count,
            Event->source_triangles,
            Event->target_triangles,
            Event->output_triangles,
            Event->status);

        FScopeLock Lock(Bridge->StatusMutex);
        *Bridge->StatusText = NewStatus;
    }

    bool ExtractMeshData(UStaticMesh* StaticMesh, FLevelSceneMeshData& OutMeshData, FString& OutError)
    {
        OutError.Empty();

        if (!StaticMesh)
        {
            OutError = TEXT("空 StaticMesh");
            return false;
        }

        const FMeshDescription* MeshDescription = StaticMesh->GetMeshDescription(0);
        if (!MeshDescription)
        {
            OutError = FString::Printf(TEXT("%s: LOD0 MeshDescription 不可用"), *StaticMesh->GetName());
            return false;
        }

        FStaticMeshConstAttributes Attributes(*MeshDescription);
        TVertexAttributesConstRef<FVector3f> VertexPositions = Attributes.GetVertexPositions();

        TMap<FVertexID, uint32> VertexRemap;

        OutMeshData.MeshName = StaticMesh->GetName();
        OutMeshData.Vertices.Reset();
        OutMeshData.Indices.Reset();
        OutMeshData.MaterialIds.Reset();

        for (const FTriangleID TriangleID : MeshDescription->Triangles().GetElementIDs())
        {
            const FPolygonGroupID PolygonGroupID = MeshDescription->GetTrianglePolygonGroup(TriangleID);

            uint32 TriangleIndices[3] = {0, 0, 0};

            for (int32 Corner = 0; Corner < 3; ++Corner)
            {
                const FVertexInstanceID VertexInstanceID = MeshDescription->GetTriangleVertexInstance(TriangleID, Corner);
                const FVertexID VertexID = MeshDescription->GetVertexInstanceVertex(VertexInstanceID);

                uint32* Existing = VertexRemap.Find(VertexID);
                if (!Existing)
                {
                    const FVector3f Position = VertexPositions[VertexID];
                    const uint32 NewIndex = static_cast<uint32>(OutMeshData.Vertices.Num() / 3);
                    OutMeshData.Vertices.Add(Position.X);
                    OutMeshData.Vertices.Add(Position.Y);
                    OutMeshData.Vertices.Add(Position.Z);
                    VertexRemap.Add(VertexID, NewIndex);
                    TriangleIndices[Corner] = NewIndex;
                }
                else
                {
                    TriangleIndices[Corner] = *Existing;
                }
            }

            if (TriangleIndices[0] == TriangleIndices[1]
                || TriangleIndices[0] == TriangleIndices[2]
                || TriangleIndices[1] == TriangleIndices[2])
            {
                continue;
            }

            OutMeshData.Indices.Append(TriangleIndices, UE_ARRAY_COUNT(TriangleIndices));
            OutMeshData.MaterialIds.Add(PolygonGroupID.GetValue());
        }

        if (OutMeshData.Indices.Num() < 3 || OutMeshData.MaterialIds.Num() == 0)
        {
            OutError = FString::Printf(TEXT("%s: 没有可用三角形"), *StaticMesh->GetName());
            return false;
        }

        if (OutMeshData.MaterialIds.Num() * 3 != OutMeshData.Indices.Num())
        {
            OutError = FString::Printf(TEXT("%s: 材质与三角面数量不一致"), *StaticMesh->GetName());
            return false;
        }

        return true;
    }

    bool CaptureCurrentLevelScene(FLevelSceneCapture& OutCapture, bool bOnlySelectedActors, FString& OutError)
    {
        OutError.Empty();
        OutCapture = FLevelSceneCapture();

        if (!IsInGameThread())
        {
            OutError = TEXT("场景采集必须在主线程执行");
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Scene] Capture called from non-game thread."));
            return false;
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Scene] Begin capture. Scope=%s"),
            bOnlySelectedActors ? TEXT("SelectedActors") : TEXT("WholeLevel"));

        if (!GEditor)
        {
            OutError = TEXT("GEditor 不可用");
            return false;
        }

        UWorld* World = GEditor->GetEditorWorldContext().World();
        if (!World)
        {
            OutError = TEXT("未找到当前编辑器世界");
            return false;
        }

        TSet<AActor*> SelectedActors;
        if (bOnlySelectedActors)
        {
            USelection* Selection = GEditor->GetSelectedActors();
            if (!Selection || Selection->Num() == 0)
            {
                OutError = TEXT("当前未选中任何 Actor，请先选择要简化的对象，或关闭“仅处理选中 Actor”");
                return false;
            }

            for (FSelectionIterator It(*Selection); It; ++It)
            {
                if (AActor* SelectedActor = Cast<AActor>(*It))
                {
                    SelectedActors.Add(SelectedActor);
                }
            }

            if (SelectedActors.Num() == 0)
            {
                OutError = TEXT("当前选择集中没有有效 Actor，请重新选择");
                return false;
            }
        }

        TArray<UStaticMeshComponent*> Components;
        Components.Reserve(512);
        int32 ScannedActors = 0;
        int32 SkippedEditorOnlyActors = 0;

        for (TActorIterator<AActor> It(World); It; ++It)
        {
            AActor* Actor = *It;
            ++ScannedActors;
            if (!IsValid(Actor) || Actor->IsEditorOnly())
            {
                ++SkippedEditorOnlyActors;
                continue;
            }

            if (bOnlySelectedActors && !SelectedActors.Contains(Actor))
            {
                continue;
            }

            TInlineComponentArray<UStaticMeshComponent*> StaticMeshComponents;
            Actor->GetComponents(StaticMeshComponents);
            for (UStaticMeshComponent* Component : StaticMeshComponents)
            {
                if (!IsValid(Component) || !Component->GetStaticMesh())
                {
                    continue;
                }
                Components.Add(Component);
            }
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Scene] Actor scan finished. scanned=%d, skipped_editor_only_or_invalid=%d, candidate_components=%d"),
            ScannedActors,
            SkippedEditorOnlyActors,
            Components.Num());

        if (Components.Num() == 0)
        {
            OutError = bOnlySelectedActors
                ? TEXT("选中的 Actor 中没有可处理的 StaticMeshComponent")
                : TEXT("当前关卡没有可处理的 StaticMeshComponent");
            return false;
        }

        TMap<UStaticMesh*, int32> MeshToIndex;
        MeshToIndex.Reserve(Components.Num());

        OutCapture.Meshes.Reserve(Components.Num());
        OutCapture.Nodes.Reserve(Components.Num());

        for (UStaticMeshComponent* Component : Components)
        {
            UStaticMesh* StaticMesh = Component->GetStaticMesh();
            if (!StaticMesh)
            {
                continue;
            }

            int32 MeshIndex = INDEX_NONE;
            if (const int32* Existing = MeshToIndex.Find(StaticMesh))
            {
                MeshIndex = *Existing;
            }
            else
            {
                FLevelSceneMeshData MeshData;
                MeshData.StaticMesh = StaticMesh;

                FString MeshError;
                if (!ExtractMeshData(StaticMesh, MeshData, MeshError))
                {
                    UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Scene] Skip mesh %s: %s"), *StaticMesh->GetName(), *MeshError);
                    continue;
                }

                MeshIndex = OutCapture.Meshes.Add(MoveTemp(MeshData));
                MeshToIndex.Add(StaticMesh, MeshIndex);
            }

            FLevelSceneNodeData Node;
            Node.MeshIndex = MeshIndex;
            Node.SourceComponent = Component;
            Node.NodeKey = Component->GetPathName();

            const FMatrix WorldMatrix = Component->GetComponentTransform().ToMatrixWithScale();
            for (int32 Row = 0; Row < 4; ++Row)
            {
                for (int32 Col = 0; Col < 4; ++Col)
                {
                    Node.WorldMatrix[Row * 4 + Col] = WorldMatrix.M[Row][Col];
                }
            }

            OutCapture.Nodes.Add(Node);
        }

        if (OutCapture.Meshes.Num() == 0 || OutCapture.Nodes.Num() == 0)
        {
            OutError = TEXT("没有可简化的网格数据（可能所有网格都不可编辑或无三角形）");
            return false;
        }

        TMap<UStaticMeshComponent*, int32> ComponentToNodeIndex;
        ComponentToNodeIndex.Reserve(OutCapture.Nodes.Num());

        for (int32 NodeIndex = 0; NodeIndex < OutCapture.Nodes.Num(); ++NodeIndex)
        {
            if (UStaticMeshComponent* Component = OutCapture.Nodes[NodeIndex].SourceComponent.Get())
            {
                ComponentToNodeIndex.Add(Component, NodeIndex);
            }
        }

        for (int32 NodeIndex = 0; NodeIndex < OutCapture.Nodes.Num(); ++NodeIndex)
        {
            UStaticMeshComponent* Component = OutCapture.Nodes[NodeIndex].SourceComponent.Get();
            if (!Component)
            {
                continue;
            }

            int32 ParentIndex = INDEX_NONE;
            USceneComponent* Parent = Component->GetAttachParent();
            while (Parent)
            {
                if (UStaticMeshComponent* ParentSM = Cast<UStaticMeshComponent>(Parent))
                {
                    if (const int32* FoundParent = ComponentToNodeIndex.Find(ParentSM))
                    {
                        ParentIndex = *FoundParent;
                        break;
                    }
                }
                Parent = Parent->GetAttachParent();
            }

            OutCapture.Nodes[NodeIndex].ParentIndex = ParentIndex;
        }

        OutCapture.RootNode = OutCapture.Nodes.IndexOfByPredicate([](const FLevelSceneNodeData& Node)
        {
            return Node.ParentIndex < 0;
        });

        if (OutCapture.RootNode == INDEX_NONE)
        {
            OutCapture.RootNode = 0;
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Scene] Capture complete. unique_meshes=%d, nodes=%d, root_node=%d"),
            OutCapture.Meshes.Num(),
            OutCapture.Nodes.Num(),
            OutCapture.RootNode);

        return true;
    }

    bool BuildMeshDescriptionFromRaw(
        const FLevelSceneMeshData& MeshData,
        const UStaticMesh* SourceMesh,
        FMeshDescription& OutMeshDescription,
        FString& OutError)
    {
        OutError.Empty();
        OutMeshDescription.Empty();

        if (MeshData.Vertices.Num() < 9 || MeshData.Indices.Num() < 3 || MeshData.MaterialIds.Num() == 0)
        {
            OutError = TEXT("简化结果为空");
            return false;
        }

        if (MeshData.Indices.Num() % 3 != 0 || MeshData.MaterialIds.Num() * 3 != MeshData.Indices.Num())
        {
            OutError = TEXT("简化结果索引/材质数据不合法");
            return false;
        }

        const int32 VertexCount = MeshData.Vertices.Num() / 3;
        const int32 TriangleCount = MeshData.Indices.Num() / 3;

        FStaticMeshAttributes Attributes(OutMeshDescription);
        Attributes.Register();

        TVertexAttributesRef<FVector3f> VertexPositions = Attributes.GetVertexPositions();
        TVertexInstanceAttributesRef<FVector3f> VertexNormals = Attributes.GetVertexInstanceNormals();
        TVertexInstanceAttributesRef<FVector3f> VertexTangents = Attributes.GetVertexInstanceTangents();
        TVertexInstanceAttributesRef<float> VertexBinormalSigns = Attributes.GetVertexInstanceBinormalSigns();
        TVertexInstanceAttributesRef<FVector4f> VertexColors = Attributes.GetVertexInstanceColors();
        TVertexInstanceAttributesRef<FVector2f> VertexUVs = Attributes.GetVertexInstanceUVs();
        VertexUVs.SetNumChannels(1);

        TArray<FVertexID> VertexIDs;
        VertexIDs.Reserve(VertexCount);

        for (int32 VertexIndex = 0; VertexIndex < VertexCount; ++VertexIndex)
        {
            const FVertexID VertexID = OutMeshDescription.CreateVertex();
            VertexIDs.Add(VertexID);

            const int32 Base = VertexIndex * 3;
            VertexPositions[VertexID] = FVector3f(
                MeshData.Vertices[Base + 0],
                MeshData.Vertices[Base + 1],
                MeshData.Vertices[Base + 2]);
        }

        TMap<int32, FPolygonGroupID> PolygonGroupMap;

        auto EnsurePolygonGroup = [&](int32 MaterialIndex) -> FPolygonGroupID
        {
            if (const FPolygonGroupID* Existing = PolygonGroupMap.Find(MaterialIndex))
            {
                return *Existing;
            }

            FPolygonGroupID GroupId;
            if (MaterialIndex >= 0)
            {
                const FPolygonGroupID Candidate(MaterialIndex);
                if (!OutMeshDescription.PolygonGroups().IsValid(Candidate))
                {
                    OutMeshDescription.CreatePolygonGroupWithID(Candidate);
                }
                GroupId = Candidate;
            }
            else
            {
                GroupId = OutMeshDescription.CreatePolygonGroup();
            }

            PolygonGroupMap.Add(MaterialIndex, GroupId);
            return GroupId;
        };

        for (int32 TriangleIndex = 0; TriangleIndex < TriangleCount; ++TriangleIndex)
        {
            const int32 I0 = static_cast<int32>(MeshData.Indices[TriangleIndex * 3 + 0]);
            const int32 I1 = static_cast<int32>(MeshData.Indices[TriangleIndex * 3 + 1]);
            const int32 I2 = static_cast<int32>(MeshData.Indices[TriangleIndex * 3 + 2]);

            if (I0 < 0 || I1 < 0 || I2 < 0 || I0 >= VertexCount || I1 >= VertexCount || I2 >= VertexCount)
            {
                continue;
            }

            if (I0 == I1 || I0 == I2 || I1 == I2)
            {
                continue;
            }

            FVertexInstanceID VertexInstanceIDs[3];
            const int32 CornerIndices[3] = { I0, I1, I2 };

            for (int32 Corner = 0; Corner < 3; ++Corner)
            {
                VertexInstanceIDs[Corner] = OutMeshDescription.CreateVertexInstance(VertexIDs[CornerIndices[Corner]]);
                VertexNormals[VertexInstanceIDs[Corner]] = FVector3f::UpVector;
                VertexTangents[VertexInstanceIDs[Corner]] = FVector3f::ForwardVector;
                VertexBinormalSigns[VertexInstanceIDs[Corner]] = 1.0f;
                VertexColors[VertexInstanceIDs[Corner]] = FVector4f(1, 1, 1, 1);
                VertexUVs.Set(VertexInstanceIDs[Corner], 0, FVector2f::ZeroVector);
            }

            const int32 MaterialIndex = (TriangleIndex < MeshData.MaterialIds.Num()) ? MeshData.MaterialIds[TriangleIndex] : 0;
            const FPolygonGroupID GroupId = EnsurePolygonGroup(MaterialIndex);

            TArray<FEdgeID> NewEdgeIDs;
            OutMeshDescription.CreateTriangle(GroupId, VertexInstanceIDs, &NewEdgeIDs);
        }

        if (OutMeshDescription.Triangles().Num() == 0)
        {
            OutError = FString::Printf(TEXT("生成 MeshDescription 失败（三角形数为 0）：%s"), SourceMesh ? *SourceMesh->GetName() : TEXT("Unknown"));
            return false;
        }

        return true;
    }

    uint64 CountSourceTriangles(const FLevelSceneCapture& Capture)
    {
        uint64 Total = 0;
        for (const FLevelSceneMeshData& MeshData : Capture.Meshes)
        {
            Total += static_cast<uint64>(MeshData.Indices.Num() / 3);
        }
        return Total;
    }

    struct FPreviewMeshLine
    {
        FString MeshName;
        uint32 SourceTriangles = 0;
        uint32 TargetTriangles = 0;
        uint32 InstanceCount = 0;
        float KeepRatio = 1.0f;
    };

    bool ComputeSceneDecisionsWithDll(
        FLevelSceneCapture& Capture,
        float TargetRatio,
        float MinMeshRatio,
        float MaxMeshRatio,
        TArray<QemSceneMeshDecision>& OutDecisions,
        uint32& OutDecisionCount,
        QemSceneSimplifyResult& OutSceneResult,
        FString& OutError)
    {
        OutError.Empty();
        OutDecisionCount = 0;
        OutSceneResult = QemSceneSimplifyResult{};

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Decision] Begin compute. meshes=%d, nodes=%d, target=%.3f, min=%.3f, max=%.3f"),
            Capture.Meshes.Num(),
            Capture.Nodes.Num(),
            TargetRatio,
            MinMeshRatio,
            MaxMeshRatio);

        if (!GFnQemSceneGraphComputeDecisions)
        {
            OutError = TEXT("DLL 未导出 qem_scene_graph_compute_decisions");
            return false;
        }

        TArray<QemSceneMeshView> MeshViews;
        MeshViews.SetNum(Capture.Meshes.Num());
        for (int32 MeshIndex = 0; MeshIndex < Capture.Meshes.Num(); ++MeshIndex)
        {
            FLevelSceneMeshData& MeshData = Capture.Meshes[MeshIndex];
            MeshViews[MeshIndex].mesh_id = static_cast<uint32>(MeshIndex);
            MeshViews[MeshIndex].mesh.vertices = MeshData.Vertices.GetData();
            MeshViews[MeshIndex].mesh.num_vertices = static_cast<uint32>(MeshData.Vertices.Num() / 3);
            MeshViews[MeshIndex].mesh.indices = MeshData.Indices.GetData();
            MeshViews[MeshIndex].mesh.num_indices = static_cast<uint32>(MeshData.Indices.Num());
            MeshViews[MeshIndex].mesh.material_ids = MeshData.MaterialIds.GetData();
            MeshViews[MeshIndex].mesh.num_attributes = 0;
            MeshViews[MeshIndex].mesh.attribute_weights = nullptr;
        }

        TArray<QemSceneGraphNodeView> NodeViews;
        NodeViews.SetNum(Capture.Nodes.Num());
        TArray<QemSceneGraphMeshBindingView> BindingViews;
        BindingViews.Reserve(Capture.Nodes.Num());

        for (int32 NodeIndex = 0; NodeIndex < Capture.Nodes.Num(); ++NodeIndex)
        {
            const FLevelSceneNodeData& Node = Capture.Nodes[NodeIndex];
            
            // The API expects parent-relative local matrices.
            // We flatten the hierarchy here because FLevelSceneNodeData already stores WorldMatrix.
            NodeViews[NodeIndex].parent_index = -1;
            FMemory::Memcpy(NodeViews[NodeIndex].local_matrix, Node.WorldMatrix, sizeof(float) * 16);

            if (Node.MeshIndex >= 0)
            {
                QemSceneGraphMeshBindingView Binding{};
                Binding.node_index = static_cast<uint32>(NodeIndex);
                Binding.mesh_index = static_cast<uint32>(Node.MeshIndex);
                Binding.use_mesh_to_node_matrix = 0;
                BindingViews.Add(Binding);
            }
        }

        QemSceneGraphView SceneView{};
        SceneView.meshes = MeshViews.GetData();
        SceneView.num_meshes = static_cast<uint32>(MeshViews.Num());
        SceneView.nodes = NodeViews.GetData();
        SceneView.num_nodes = static_cast<uint32>(NodeViews.Num());
        SceneView.mesh_bindings = BindingViews.GetData();
        SceneView.num_mesh_bindings = static_cast<uint32>(BindingViews.Num());

        QemScenePolicy Policy{};
        Policy.target_triangle_ratio = FMath::Clamp(TargetRatio, 0.01f, 1.0f);
        Policy.min_mesh_ratio = FMath::Clamp(MinMeshRatio, 0.0f, 1.0f);
        Policy.max_mesh_ratio = FMath::Clamp(MaxMeshRatio, Policy.min_mesh_ratio, 1.0f);
        Policy.weight_mode = QEM_SCENE_WEIGHT_MESH_VOLUME_X_INSTANCES;
        Policy.use_world_scale = 1;
        Policy.target_total_triangles = 0;
        Policy.min_triangles_per_mesh = 16;
        Policy.weight_exponent = 1.15f;
        Policy.enable_parallel = 1;
        Policy.max_parallel_tasks = 0;
        Policy.external_importance_weights = nullptr;
        Policy.external_importance_count = 0;

        OutDecisions.SetNum(MeshViews.Num());

        const int32 Status = GFnQemSceneGraphComputeDecisions(
            &SceneView,
            &Policy,
            OutDecisions.GetData(),
            static_cast<uint32>(OutDecisions.Num()),
            &OutDecisionCount,
            &OutSceneResult);

        if (Status != QEM_STATUS_SUCCESS || OutSceneResult.status != QEM_STATUS_SUCCESS)
        {
            UE_LOG(
                LogQEMLevelSceneSimplifier,
                Error,
                TEXT("[Decision] Compute failed. status=%d, result=%d, source=%llu, target=%llu"),
                Status,
                OutSceneResult.status,
                OutSceneResult.source_triangles,
                OutSceneResult.target_triangles);
            OutError = FString::Printf(TEXT("qem_scene_graph_compute_decisions 失败：status=%d, result=%d"), Status, OutSceneResult.status);
            return false;
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Decision] Compute complete. decision_count=%u, source=%llu, target=%llu"),
            OutDecisionCount,
            OutSceneResult.source_triangles,
            OutSceneResult.target_triangles);

        return true;
    }

    FString BuildPreviewSummary(
        const FLevelSceneCapture& Capture,
        const TArray<QemSceneMeshDecision>& Decisions,
        uint32 DecisionCount,
        float TargetRatio,
        float MinMeshRatio,
        float MaxMeshRatio,
        bool bOnlySelectedActors)
    {
        const float ClampedTargetRatio = FMath::Clamp(TargetRatio, 0.01f, 1.0f);
        const float ClampedMinRatio = FMath::Clamp(MinMeshRatio, 0.0f, 1.0f);
        const float ClampedMaxRatio = FMath::Clamp(MaxMeshRatio, ClampedMinRatio, 1.0f);

        TArray<uint32> InstanceCounts;
        InstanceCounts.Init(0, Capture.Meshes.Num());
        for (const FLevelSceneNodeData& Node : Capture.Nodes)
        {
            if (Node.MeshIndex >= 0 && Node.MeshIndex < Capture.Meshes.Num())
            {
                InstanceCounts[Node.MeshIndex] += 1;
            }
        }

        uint64 SourceTrianglesUnique = 0;
        uint64 TargetTrianglesUnique = 0;
        double SourceTrianglesWeighted = 0.0;
        double TargetTrianglesWeighted = 0.0;

        TArray<FPreviewMeshLine> PreviewLines;
        PreviewLines.Reserve(Capture.Meshes.Num());

        TArray<const QemSceneMeshDecision*> DecisionsByMesh;
        DecisionsByMesh.Init(nullptr, Capture.Meshes.Num());
        for (uint32 Index = 0; Index < DecisionCount && Index < static_cast<uint32>(Decisions.Num()); ++Index)
        {
            const QemSceneMeshDecision& Decision = Decisions[Index];
            if (Decision.mesh_index < static_cast<uint32>(Capture.Meshes.Num()))
            {
                DecisionsByMesh[Decision.mesh_index] = &Decision;
            }
        }

        for (int32 MeshIndex = 0; MeshIndex < Capture.Meshes.Num(); ++MeshIndex)
        {
            const FLevelSceneMeshData& MeshData = Capture.Meshes[MeshIndex];
            const uint32 SourceTriangles = static_cast<uint32>(MeshData.Indices.Num() / 3);
            if (SourceTriangles == 0)
            {
                continue;
            }

            const uint32 InstanceCount = FMath::Max(1u, InstanceCounts[MeshIndex]);
            const QemSceneMeshDecision* Decision = DecisionsByMesh[MeshIndex];
            const float KeepRatio = Decision
                ? FMath::Clamp(Decision->keep_ratio, ClampedMinRatio, ClampedMaxRatio)
                : FMath::Clamp(ClampedTargetRatio, ClampedMinRatio, ClampedMaxRatio);

            uint32 TargetTriangles = Decision
                ? Decision->target_triangles
                : static_cast<uint32>(FMath::RoundToInt(static_cast<float>(SourceTriangles) * KeepRatio));
            TargetTriangles = FMath::Clamp(TargetTriangles, 2u, SourceTriangles);

            SourceTrianglesUnique += SourceTriangles;
            TargetTrianglesUnique += TargetTriangles;

            SourceTrianglesWeighted += static_cast<double>(SourceTriangles) * static_cast<double>(InstanceCount);
            TargetTrianglesWeighted += static_cast<double>(TargetTriangles) * static_cast<double>(InstanceCount);

            FPreviewMeshLine& Line = PreviewLines.AddDefaulted_GetRef();
            Line.MeshName = MeshData.MeshName;
            Line.SourceTriangles = SourceTriangles;
            Line.TargetTriangles = TargetTriangles;
            Line.InstanceCount = InstanceCount;
            Line.KeepRatio = KeepRatio;
        }

        PreviewLines.Sort([](const FPreviewMeshLine& A, const FPreviewMeshLine& B)
        {
            return A.SourceTriangles > B.SourceTriangles;
        });

        const double UniqueKeepRatio = (SourceTrianglesUnique > 0)
            ? static_cast<double>(TargetTrianglesUnique) / static_cast<double>(SourceTrianglesUnique)
            : 1.0;

        const double WeightedKeepRatio = (SourceTrianglesWeighted > 0.0)
            ? (TargetTrianglesWeighted / SourceTrianglesWeighted)
            : 1.0;

        FString Summary = FString::Printf(
            TEXT("预览完成（不执行简化/不写回）｜范围：%s｜唯一网格=%d 节点=%d\n")
            TEXT("参数：target=%.2f min=%.2f max=%.2f\n")
            TEXT("唯一三角：%llu -> %llu（保留 %.1f%%）｜实例加权：%.0f -> %.0f（保留 %.1f%%）"),
            bOnlySelectedActors ? TEXT("仅选中") : TEXT("整个关卡"),
            Capture.Meshes.Num(),
            Capture.Nodes.Num(),
            ClampedTargetRatio,
            ClampedMinRatio,
            ClampedMaxRatio,
            SourceTrianglesUnique,
            TargetTrianglesUnique,
            UniqueKeepRatio * 100.0,
            SourceTrianglesWeighted,
            TargetTrianglesWeighted,
            WeightedKeepRatio * 100.0);

        const int32 MaxPreviewLines = FMath::Min(8, PreviewLines.Num());
        for (int32 Index = 0; Index < MaxPreviewLines; ++Index)
        {
            const FPreviewMeshLine& Line = PreviewLines[Index];
            Summary += FString::Printf(
                TEXT("\n[%d] %s: %u -> %u (%.1f%%), inst=%u"),
                Index + 1,
                *Line.MeshName,
                Line.SourceTriangles,
                Line.TargetTriangles,
                static_cast<double>(Line.KeepRatio) * 100.0,
                Line.InstanceCount);
        }

        return Summary;
    }

    void BuildSceneTreeSnapshot(
        const FLevelSceneCapture& Capture,
        const TArray<QemSceneMeshDecision>* Decisions,
        uint32 DecisionCount,
        const TMap<FString, float>& NodeKeepRatioLimits,
        TArray<FQEMSceneTreeNodeView>& OutNodes,
        uint64& OutTargetTriangles)
    {
        OutTargetTriangles = 0;
        OutNodes.Reset();

        if (!IsInGameThread())
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[SceneTree] BuildSceneTreeSnapshot must run on game thread."));
            return;
        }

        OutNodes.Reserve(Capture.Nodes.Num());

        TArray<const QemSceneMeshDecision*> DecisionsByMesh;
        if (Decisions)
        {
            DecisionsByMesh.Init(nullptr, Capture.Meshes.Num());
            for (uint32 Index = 0; Index < DecisionCount && Index < static_cast<uint32>(Decisions->Num()); ++Index)
            {
                const QemSceneMeshDecision& Decision = (*Decisions)[Index];
                if (Decision.mesh_index < static_cast<uint32>(Capture.Meshes.Num()))
                {
                    DecisionsByMesh[Decision.mesh_index] = &Decision;
                }
            }
        }

        for (int32 NodeIndex = 0; NodeIndex < Capture.Nodes.Num(); ++NodeIndex)
        {
            const FLevelSceneNodeData& Node = Capture.Nodes[NodeIndex];

            FString ActorName = TEXT("UnknownActor");
            FString ComponentName = TEXT("UnknownComponent");
            if (UStaticMeshComponent* Component = Node.SourceComponent.Get())
            {
                ComponentName = Component->GetName();
                if (AActor* Owner = Component->GetOwner())
                {
                    ActorName = Owner->GetName();
                }
            }

            FString MeshName = TEXT("NoMesh");
            uint32 SourceTriangles = 0;
            uint32 TargetTriangles = 0;
            float KeepRatio = 1.0f;
            bool bHasLimit = false;
            float LimitKeepRatio = 1.0f;

            if (Node.MeshIndex >= 0 && Node.MeshIndex < Capture.Meshes.Num())
            {
                const FLevelSceneMeshData& MeshData = Capture.Meshes[Node.MeshIndex];
                MeshName = MeshData.MeshName;
                SourceTriangles = static_cast<uint32>(MeshData.Indices.Num() / 3);

                TargetTriangles = SourceTriangles;
                if (Decisions)
                {
                    if (const QemSceneMeshDecision* Decision = DecisionsByMesh[Node.MeshIndex])
                    {
                        if (SourceTriangles >= 2)
                        {
                            TargetTriangles = FMath::Clamp(Decision->target_triangles, 2u, SourceTriangles);
                        }
                        KeepRatio = (SourceTriangles > 0)
                            ? FMath::Clamp(static_cast<float>(TargetTriangles) / static_cast<float>(SourceTriangles), 0.0f, 1.0f)
                            : 1.0f;
                    }
                }
                else if (SourceTriangles > 0)
                {
                    KeepRatio = 1.0f;
                }

                if (const float* Limit = NodeKeepRatioLimits.Find(Node.NodeKey))
                {
                    bHasLimit = true;
                    LimitKeepRatio = FMath::Clamp(*Limit, 0.0f, 1.0f);

                    if (SourceTriangles > 0)
                    {
                        uint32 LimitedTarget = SourceTriangles;
                        if (SourceTriangles >= 2)
                        {
                            LimitedTarget = FMath::Clamp(
                                static_cast<uint32>(FMath::CeilToInt(static_cast<float>(SourceTriangles) * LimitKeepRatio)),
                                2u,
                                SourceTriangles);
                        }
                        TargetTriangles = FMath::Min(TargetTriangles, LimitedTarget);
                        KeepRatio = FMath::Clamp(static_cast<float>(TargetTriangles) / static_cast<float>(SourceTriangles), 0.0f, 1.0f);
                    }
                }
            }

            FQEMSceneTreeNodeView& View = OutNodes.AddDefaulted_GetRef();
            View.NodeIndex = NodeIndex;
            View.ParentIndex = Node.ParentIndex;
            View.NodeKey = Node.NodeKey;
            View.DisplayName = FString::Printf(TEXT("%s/%s [%s]"), *ActorName, *ComponentName, *MeshName);
            View.SourceTriangles = SourceTriangles;
            View.TargetTriangles = TargetTriangles;
            View.KeepRatio = KeepRatio;
            View.bHasLimit = bHasLimit;
            View.LimitKeepRatio = LimitKeepRatio;

            OutTargetTriangles += TargetTriangles;
        }
    }

    int32 ApplyNodeLimitsToDecisions(
        const FLevelSceneCapture& Capture,
        const TMap<FString, float>& NodeKeepRatioLimits,
        TArray<QemSceneMeshDecision>& InOutDecisions,
        uint32 DecisionCount)
    {
        if (Capture.Meshes.Num() == 0 || NodeKeepRatioLimits.Num() == 0)
        {
            return 0;
        }

        TArray<float> MeshMaxKeep;
        MeshMaxKeep.Init(1.0f, Capture.Meshes.Num());

        for (const FLevelSceneNodeData& Node : Capture.Nodes)
        {
            if (Node.MeshIndex < 0 || Node.MeshIndex >= Capture.Meshes.Num())
            {
                continue;
            }

            if (const float* Limit = NodeKeepRatioLimits.Find(Node.NodeKey))
            {
                MeshMaxKeep[Node.MeshIndex] = FMath::Min(MeshMaxKeep[Node.MeshIndex], FMath::Clamp(*Limit, 0.0f, 1.0f));
            }
        }

        int32 LimitedMeshCount = 0;
        for (uint32 Index = 0; Index < DecisionCount && Index < static_cast<uint32>(InOutDecisions.Num()); ++Index)
        {
            QemSceneMeshDecision& Decision = InOutDecisions[Index];
            if (Decision.mesh_index >= static_cast<uint32>(Capture.Meshes.Num()))
            {
                continue;
            }

            const float LimitKeepRatio = MeshMaxKeep[Decision.mesh_index];
            if (LimitKeepRatio >= 1.0f)
            {
                continue;
            }

            const uint32 SourceTriangles = Decision.source_triangles;
            if (SourceTriangles == 0)
            {
                continue;
            }

            uint32 LimitedTarget = SourceTriangles;
            if (SourceTriangles >= 2)
            {
                LimitedTarget = FMath::Clamp(
                    static_cast<uint32>(FMath::CeilToInt(static_cast<float>(SourceTriangles) * LimitKeepRatio)),
                    2u,
                    SourceTriangles);
            }

            if (LimitedTarget < Decision.target_triangles)
            {
                Decision.target_triangles = LimitedTarget;
                Decision.keep_ratio = FMath::Clamp(static_cast<float>(LimitedTarget) / static_cast<float>(SourceTriangles), 0.0f, 1.0f);
                ++LimitedMeshCount;
            }
        }

        return LimitedMeshCount;
    }

    bool ApplySimplifiedMeshesWithMeshReduction(
        FLevelSceneCapture& Capture,
        const TArray<QemSceneMeshDecision>& Decisions,
        uint32 DecisionCount,
        int32& OutUpdatedMeshCount,
        uint64& OutOutputTriangles,
        FString& OutReducerVersion,
        FString& OutError,
        const FStageProgressBridge* ProgressBridge = nullptr)
    {
        OutError.Empty();
        OutReducerVersion.Empty();
        OutUpdatedMeshCount = 0;
        OutOutputTriangles = 0;

        if (!IsInGameThread())
        {
            OutError = TEXT("Mesh 写回必须在主线程执行");
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Apply] Called from non-game thread."));
            return false;
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Apply] Begin apply. meshes=%d, decisions=%d, decision_count=%u"),
            Capture.Meshes.Num(),
            Decisions.Num(),
            DecisionCount);

        IMeshReductionManagerModule& MeshReductionModule = FModuleManager::LoadModuleChecked<IMeshReductionManagerModule>(TEXT("MeshReductionInterface"));
        IMeshReduction* MeshReduction = MeshReductionModule.GetStaticMeshReductionInterface();
        if (!MeshReduction)
        {
            OutError = TEXT("未获取到 UE MeshReduction 接口（GetStaticMeshReductionInterface 返回空）");
            return false;
        }
        OutReducerVersion = MeshReduction->GetVersionString();
        UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Apply] MeshReduction ready. version=%s"), *OutReducerVersion);

        TArray<const QemSceneMeshDecision*> DecisionsByMesh;
        DecisionsByMesh.Init(nullptr, Capture.Meshes.Num());
        for (uint32 Index = 0; Index < DecisionCount && Index < static_cast<uint32>(Decisions.Num()); ++Index)
        {
            const QemSceneMeshDecision& Decision = Decisions[Index];
            if (Decision.mesh_index < static_cast<uint32>(Capture.Meshes.Num()))
            {
                DecisionsByMesh[Decision.mesh_index] = &Decision;
            }
        }

        const int32 TotalMeshes = FMath::Max(1, Capture.Meshes.Num());

        for (int32 MeshIndex = 0; MeshIndex < Capture.Meshes.Num(); ++MeshIndex)
        {
            FLevelSceneMeshData& MeshData = Capture.Meshes[MeshIndex];
            UStaticMesh* StaticMesh = MeshData.StaticMesh.Get();
            const float LoopPctStart = static_cast<float>(MeshIndex) / static_cast<float>(TotalMeshes);
            const float LoopPctEnd = static_cast<float>(MeshIndex + 1) / static_cast<float>(TotalMeshes);
            const FString MeshNameForStatus = StaticMesh ? StaticMesh->GetName() : FString::Printf(TEXT("mesh_index_%d"), MeshIndex);

            UpdateStageProgress(
                ProgressBridge,
                LoopPctStart,
                FString::Printf(TEXT("简化中 %d/%d：%s"), MeshIndex + 1, TotalMeshes, *MeshNameForStatus));

            if (!StaticMesh)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Verbose, TEXT("[Apply] Skip mesh_index=%d: StaticMesh expired"), MeshIndex);
                UpdateStageProgress(
                    ProgressBridge,
                    LoopPctEnd,
                    FString::Printf(TEXT("简化中 %d/%d：跳过失效网格"), MeshIndex + 1, TotalMeshes));
                continue;
            }

            const FMeshDescription* SourceMeshDescription = StaticMesh->GetMeshDescription(0);
            if (!SourceMeshDescription)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Apply] Skip %s: no LOD0 MeshDescription"), *StaticMesh->GetName());
                UpdateStageProgress(
                    ProgressBridge,
                    LoopPctEnd,
                    FString::Printf(TEXT("简化中 %d/%d：%s 缺少 MeshDescription，已跳过"), MeshIndex + 1, TotalMeshes, *StaticMesh->GetName()));
                continue;
            }

            const uint32 SourceTriangles = static_cast<uint32>(SourceMeshDescription->Triangles().Num());
            if (SourceTriangles < 2)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Verbose, TEXT("[Apply] Skip %s: source_triangles=%u < 2"), *StaticMesh->GetName(), SourceTriangles);
                UpdateStageProgress(
                    ProgressBridge,
                    LoopPctEnd,
                    FString::Printf(TEXT("简化中 %d/%d：%s 三角形过少，已跳过"), MeshIndex + 1, TotalMeshes, *StaticMesh->GetName()));
                continue;
            }

            uint32 TargetTriangles = SourceTriangles;
            if (const QemSceneMeshDecision* Decision = DecisionsByMesh[MeshIndex])
            {
                TargetTriangles = Decision->target_triangles;
            }
            TargetTriangles = FMath::Clamp(TargetTriangles, 2u, SourceTriangles);

            if (TargetTriangles >= SourceTriangles)
            {
                UE_LOG(
                    LogQEMLevelSceneSimplifier,
                    Verbose,
                    TEXT("[Apply] Keep %s unchanged. source=%u, target=%u"),
                    *StaticMesh->GetName(),
                    SourceTriangles,
                    TargetTriangles);
                OutOutputTriangles += SourceTriangles;
                UpdateStageProgress(
                    ProgressBridge,
                    LoopPctEnd,
                    FString::Printf(TEXT("简化中 %d/%d：%s 无需简化"), MeshIndex + 1, TotalMeshes, *StaticMesh->GetName()));
                continue;
            }

            FMeshReductionSettings ReductionSettings;
            ReductionSettings.TerminationCriterion = EStaticMeshReductionTerimationCriterion::Triangles;
            ReductionSettings.PercentTriangles = FMath::Clamp(static_cast<float>(TargetTriangles) / static_cast<float>(SourceTriangles), 0.0f, 1.0f);
            ReductionSettings.MaxNumOfTriangles = TargetTriangles;
            ReductionSettings.PercentVertices = 1.0f;
            ReductionSettings.MaxNumOfVerts = TNumericLimits<uint32>::Max();
            ReductionSettings.MaxDeviation = 0.0f;

            FOverlappingCorners OverlappingCorners;
            FStaticMeshOperations::FindOverlappingCorners(OverlappingCorners, *SourceMeshDescription, THRESH_POINTS_ARE_SAME);

            FMeshDescription ReducedMeshDescription;
            float MaxDeviation = 0.0f;
            MeshReduction->ReduceMeshDescription(
                ReducedMeshDescription,
                MaxDeviation,
                *SourceMeshDescription,
                OverlappingCorners,
                ReductionSettings);

            const uint32 ReducedTriangles = static_cast<uint32>(ReducedMeshDescription.Triangles().Num());
            if (ReducedTriangles == 0)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Apply] Reduce returned 0 triangles for %s, keeping source=%u"), *StaticMesh->GetName(), SourceTriangles);
                OutOutputTriangles += SourceTriangles;
                UpdateStageProgress(
                    ProgressBridge,
                    LoopPctEnd,
                    FString::Printf(TEXT("简化中 %d/%d：%s 输出为空，保留原网格"), MeshIndex + 1, TotalMeshes, *StaticMesh->GetName()));
                continue;
            }

            StaticMesh->Modify();
            StaticMesh->CreateMeshDescription(0, MoveTemp(ReducedMeshDescription));
            StaticMesh->CommitMeshDescription(0);
            StaticMesh->Build(false);
            StaticMesh->MarkPackageDirty();
            StaticMesh->PostEditChange();

            OutOutputTriangles += ReducedTriangles;
            ++OutUpdatedMeshCount;

            UE_LOG(
                LogQEMLevelSceneSimplifier,
                Log,
                TEXT("[Apply] Updated %s. source=%u -> reduced=%u"),
                *StaticMesh->GetName(),
                SourceTriangles,
                ReducedTriangles);

            UpdateStageProgress(
                ProgressBridge,
                LoopPctEnd,
                FString::Printf(TEXT("简化中 %d/%d：%s %u->%u tris"), MeshIndex + 1, TotalMeshes, *StaticMesh->GetName(), SourceTriangles, ReducedTriangles));
        }

        UpdateStageProgress(ProgressBridge, 1.0f, TEXT("简化阶段完成"));

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Apply] Finished. updated_meshes=%d, output_triangles=%llu"),
            OutUpdatedMeshCount,
            OutOutputTriangles);

        return true;
    }

    bool BackupMeshesBeforeApply(const FLevelSceneCapture& Capture, FBackupResult& OutBackupResult, FString& OutError, const FStageProgressBridge* ProgressBridge = nullptr)
    {
        OutError.Empty();
        OutBackupResult = FBackupResult();

        if (!IsInGameThread())
        {
            OutError = TEXT("备份必须在主线程执行");
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Backup] Called from non-game thread."));
            return false;
        }

        UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Backup] Begin backup. meshes=%d"), Capture.Meshes.Num());

        if (Capture.Meshes.Num() == 0)
        {
            OutError = TEXT("无可备份网格");
            return false;
        }

        IAssetTools& AssetTools = FModuleManager::LoadModuleChecked<FAssetToolsModule>(TEXT("AssetTools")).Get();

        OutBackupResult.BackupFolder = FString::Printf(
            TEXT("/Game/QEMBackups/%s"),
            *FDateTime::Now().ToString(TEXT("%Y%m%d_%H%M%S")));

        const int32 TotalMeshes = FMath::Max(1, Capture.Meshes.Num());

        for (int32 MeshIndex = 0; MeshIndex < Capture.Meshes.Num(); ++MeshIndex)
        {
            const FLevelSceneMeshData& MeshData = Capture.Meshes[MeshIndex];
            UStaticMesh* StaticMesh = MeshData.StaticMesh.Get();
            const float LoopPctStart = static_cast<float>(MeshIndex) / static_cast<float>(TotalMeshes);
            const float LoopPctEnd = static_cast<float>(MeshIndex + 1) / static_cast<float>(TotalMeshes);

            UpdateStageProgress(
                ProgressBridge,
                LoopPctStart,
                FString::Printf(TEXT("备份中 %d/%d"), MeshIndex + 1, TotalMeshes));

            if (!StaticMesh)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Verbose, TEXT("[Backup] Skip expired mesh reference."));
                UpdateStageProgress(
                    ProgressBridge,
                    LoopPctEnd,
                    FString::Printf(TEXT("备份中 %d/%d：跳过失效网格"), MeshIndex + 1, TotalMeshes));
                continue;
            }

            ++OutBackupResult.Attempted;

            const FString BaseAssetName = FString::Printf(TEXT("%s_QEMBackup"), *StaticMesh->GetName());
            const FString DesiredPackageName = OutBackupResult.BackupFolder / BaseAssetName;

            FString UniquePackageName;
            FString UniqueAssetName;
            AssetTools.CreateUniqueAssetName(DesiredPackageName, TEXT(""), UniquePackageName, UniqueAssetName);

            const FString UniquePackagePath = FPackageName::GetLongPackagePath(UniquePackageName);
            UObject* Duplicated = AssetTools.DuplicateAsset(UniqueAssetName, UniquePackagePath, StaticMesh);
            if (!Duplicated)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Backup] Duplicate failed. source=%s, target=%s"), *StaticMesh->GetName(), *UniquePackageName);
                OutError = FString::Printf(
                    TEXT("备份失败：%s（目标包：%s）"),
                    *StaticMesh->GetName(),
                    *UniquePackageName);
                return false;
            }

            Duplicated->MarkPackageDirty();
            ++OutBackupResult.Succeeded;
            UE_LOG(LogQEMLevelSceneSimplifier, Verbose, TEXT("[Backup] Duplicated %s -> %s"), *StaticMesh->GetName(), *UniquePackageName);

            UpdateStageProgress(
                ProgressBridge,
                LoopPctEnd,
                FString::Printf(TEXT("备份中 %d/%d：%s"), MeshIndex + 1, TotalMeshes, *StaticMesh->GetName()));
        }

        if (OutBackupResult.Attempted == 0)
        {
            OutError = TEXT("未找到可备份网格");
            return false;
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Backup] Complete. attempted=%d, succeeded=%d, folder=%s"),
            OutBackupResult.Attempted,
            OutBackupResult.Succeeded,
            *OutBackupResult.BackupFolder);

        UpdateStageProgress(ProgressBridge, 1.0f, TEXT("备份阶段完成"));

        return true;
    }

    class SQEMLevelSceneSimplifierPanel : public SCompoundWidget
    {
    public:
        SLATE_BEGIN_ARGS(SQEMLevelSceneSimplifierPanel) {}
            SLATE_ARGUMENT(FQEMLevelSceneSimplifierModule*, Module)
        SLATE_END_ARGS()

        struct FSceneTreeItem;
        using FSceneTreeItemPtr = TSharedPtr<FSceneTreeItem>;

        struct FSceneTreeItem
        {
            FQEMSceneTreeNodeView Node;
            TArray<FSceneTreeItemPtr> Children;
        };

        void Construct(const FArguments& InArgs)
        {
            Module = InArgs._Module;

            TargetRatio = 0.5f;
            MinMeshRatio = 0.05f;
            MaxMeshRatio = 1.0f;
            bOnlySelectedActors = false;
            SelectedNodeLimitRatio = 0.5f;

            ChildSlot
            [
                SNew(SVerticalBox)

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8)
                [
                    SNew(STextBlock)
                    .Text(LOCTEXT("PanelTitle", "QEM 关卡场景简化（DLL）"))
                    .Font(FCoreStyle::GetDefaultFontStyle("Bold", 12))
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 4)
                [
                    SNew(STextBlock)
                    .Text(LOCTEXT("DllPathLabel", "DLL 路径（可选，留空自动从插件目录搜索）"))
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 0, 8, 6)
                [
                    SAssignNew(DllPathTextBox, SEditableTextBox)
                    .HintText(LOCTEXT("DllPathHint", "例如：D:/.../qem_simplifier.dll"))
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 4)
                [
                    BuildNumericRow(
                        LOCTEXT("TargetRatio", "场景目标保留比例"),
                        [this]() -> TOptional<float> { return TargetRatio; },
                        0.01f,
                        1.0f,
                        [this](float NewValue) { SetTargetRatio(NewValue); })
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    BuildNumericRow(
                        LOCTEXT("MinMeshRatio", "单网格最小保留比例"),
                        [this]() -> TOptional<float> { return MinMeshRatio; },
                        0.0f,
                        1.0f,
                        [this](float NewValue) { SetMinMeshRatioWithAutoFix(NewValue); })
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    BuildNumericRow(
                        LOCTEXT("MaxMeshRatio", "单网格最大保留比例"),
                        [this]() -> TOptional<float> { return MaxMeshRatio; },
                        0.0f,
                        1.0f,
                        [this](float NewValue) { SetMaxMeshRatioWithAutoFix(NewValue); })
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 8)
                [
                    SNew(SCheckBox)
                    .IsChecked_Lambda([this]()
                    {
                        return bOnlySelectedActors ? ECheckBoxState::Checked : ECheckBoxState::Unchecked;
                    })
                    .OnCheckStateChanged_Lambda([this](ECheckBoxState NewState)
                    {
                        bOnlySelectedActors = (NewState == ECheckBoxState::Checked);
                    })
                    [
                        SNew(STextBlock)
                        .Text(LOCTEXT("OnlySelectedActors", "仅处理选中 Actor（未勾选则处理整个关卡）"))
                    ]
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(SButton)
                    .Text(LOCTEXT("ComputeParams", "1) 计算简化参数（不写回）"))
                    .IsEnabled_Lambda([this]()
                    {
                        return Module && !Module->IsRunning();
                    })
                    .OnClicked_Lambda([this]()
                    {
                        if (!Module)
                        {
                            return FReply::Handled();
                        }

                        const float ClampedMin = FMath::Clamp(MinMeshRatio, 0.0f, 1.0f);
                        const float ClampedMax = FMath::Clamp(MaxMeshRatio, ClampedMin, 1.0f);
                        const FString DllPath = DllPathTextBox.IsValid() ? DllPathTextBox->GetText().ToString() : FString();

                        Module->ComputeSimplifyParameters(
                            FMath::Clamp(TargetRatio, 0.01f, 1.0f),
                            ClampedMin,
                            ClampedMax,
                            bOnlySelectedActors,
                            DllPath);

                        RefreshSceneTreeFromModule();

                        return FReply::Handled();
                    })
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(SButton)
                    .Text(LOCTEXT("ApplyComputed", "2) 执行简化（使用已计算参数）"))
                    .IsEnabled_Lambda([this]()
                    {
                        return Module && !Module->IsRunning() && Module->HasComputedPlan();
                    })
                    .OnClicked_Lambda([this]()
                    {
                        if (!Module)
                        {
                            return FReply::Handled();
                        }

                        Module->ApplyComputedSimplification();
                        RefreshSceneTreeFromModule();

                        return FReply::Handled();
                    })
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 8, 8, 2)
                [
                    SNew(SSeparator)
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 4, 8, 2)
                [
                    SNew(STextBlock)
                    .Text(LOCTEXT("SceneGraphTitle", "场景图结构（窗口打开自动扫描，可选中节点设置限制）"))
                    .Font(FCoreStyle::GetDefaultFontStyle("Bold", 10))
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(SButton)
                    .Text(LOCTEXT("RescanNodes", "刷新关卡节点树"))
                    .IsEnabled_Lambda([this]()
                    {
                        return Module && !Module->IsRunning();
                    })
                    .OnClicked_Lambda([this]()
                    {
                        if (Module)
                        {
                            Module->ScanCurrentLevelNodes(false);
                            RefreshSceneTreeFromModule();
                        }
                        return FReply::Handled();
                    })
                ]

                + SVerticalBox::Slot()
                .FillHeight(1.0f)
                .Padding(8, 2, 8, 6)
                [
                    SNew(SBox)
                    .MinDesiredHeight(220.0f)
                    [
                        SAssignNew(SceneTreeView, STreeView<FSceneTreeItemPtr>)
                        .TreeItemsSource(&RootTreeItems)
                        .SelectionMode(ESelectionMode::Single)
                        .OnGenerateRow(this, &SQEMLevelSceneSimplifierPanel::OnGenerateSceneTreeRow)
                        .OnGetChildren(this, &SQEMLevelSceneSimplifierPanel::OnGetSceneTreeChildren)
                        .OnSelectionChanged(this, &SQEMLevelSceneSimplifierPanel::OnSceneTreeSelectionChanged)
                    ]
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(STextBlock)
                    .Text_Lambda([this]()
                    {
                        if (!SelectedNodeKey.IsEmpty())
                        {
                            return FText::FromString(FString::Printf(TEXT("当前选中节点：%s"), *SelectedNodeDisplayName));
                        }
                        return LOCTEXT("NoNodeSelected", "当前未选中节点");
                    })
                    .AutoWrapText(true)
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(SHorizontalBox)
                    + SHorizontalBox::Slot()
                    .AutoWidth()
                    .VAlign(VAlign_Center)
                    [
                        SNew(STextBlock)
                        .Text(LOCTEXT("SelectedNodeLimitRatio", "选中节点最大保留比例限制"))
                    ]
                    + SHorizontalBox::Slot()
                    .Padding(10, 0, 0, 0)
                    .FillWidth(1.0f)
                    [
                        SAssignNew(SelectedNodeLimitEntry, SNumericEntryBox<float>)
                        .AllowSpin(true)
                        .MinValue(0.0f)
                        .MaxValue(1.0f)
                        .Value_Lambda([this]() -> TOptional<float>
                        {
                            return SelectedNodeLimitRatio;
                        })
                        .OnValueChanged_Lambda([this](float NewValue)
                        {
                            SelectedNodeLimitRatio = NewValue;
                        })
                    ]
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(SHorizontalBox)
                    + SHorizontalBox::Slot()
                    .AutoWidth()
                    .Padding(0, 0, 6, 0)
                    [
                        SNew(SButton)
                        .Text(LOCTEXT("ApplySelectedNodeLimit", "应用到选中节点"))
                        .IsEnabled_Lambda([this]()
                        {
                            return Module && !Module->IsRunning() && !SelectedNodeKey.IsEmpty();
                        })
                        .OnClicked_Lambda([this]()
                        {
                            if (Module && !SelectedNodeKey.IsEmpty())
                            {
                                Module->SetNodeKeepRatioLimit(SelectedNodeKey, FMath::Clamp(SelectedNodeLimitRatio, 0.0f, 1.0f));
                                RefreshSceneTreeFromModule();
                            }
                            return FReply::Handled();
                        })
                    ]
                    + SHorizontalBox::Slot()
                    .AutoWidth()
                    .Padding(0, 0, 6, 0)
                    [
                        SNew(SButton)
                        .Text(LOCTEXT("ClearSelectedNodeLimit", "清除选中节点限制"))
                        .IsEnabled_Lambda([this]()
                        {
                            return Module && !Module->IsRunning() && !SelectedNodeKey.IsEmpty();
                        })
                        .OnClicked_Lambda([this]()
                        {
                            if (Module && !SelectedNodeKey.IsEmpty())
                            {
                                Module->ClearNodeKeepRatioLimit(SelectedNodeKey);
                                RefreshSceneTreeFromModule();
                            }
                            return FReply::Handled();
                        })
                    ]
                    + SHorizontalBox::Slot()
                    .AutoWidth()
                    [
                        SNew(SButton)
                        .Text(LOCTEXT("ClearAllNodeLimit", "清除全部节点限制"))
                        .IsEnabled_Lambda([this]()
                        {
                            return Module && !Module->IsRunning();
                        })
                        .OnClicked_Lambda([this]()
                        {
                            if (Module)
                            {
                                Module->ClearAllNodeKeepRatioLimits();
                                RefreshSceneTreeFromModule();
                            }
                            return FReply::Handled();
                        })
                    ]
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 2)
                [
                    SNew(SProgressBar)
                    .Percent_Lambda([this]() -> TOptional<float>
                    {
                        return Module ? Module->GetProgress() : 0.0f;
                    })
                ]

                + SVerticalBox::Slot()
                .AutoHeight()
                .Padding(8, 4, 8, 8)
                [
                    SNew(STextBlock)
                    .Text_Lambda([this]()
                    {
                        return FText::FromString(Module ? Module->GetStatusText() : TEXT("模块未就绪"));
                    })
                    .AutoWrapText(true)
                ]
            ];

            if (Module)
            {
                Module->ScanCurrentLevelNodes(false);
            }
            RefreshSceneTreeFromModule();
            RegisterActiveTimer(0.4f, FWidgetActiveTimerDelegate::CreateSP(this, &SQEMLevelSceneSimplifierPanel::HandleAutoRefresh));
        }

    private:
        void SetTargetRatio(float NewValue)
        {
            TargetRatio = FMath::Clamp(NewValue, 0.01f, 1.0f);
        }

        void SetMinMeshRatioWithAutoFix(float NewValue)
        {
            MinMeshRatio = FMath::Clamp(NewValue, 0.0f, 1.0f);
            if (MinMeshRatio > MaxMeshRatio)
            {
                MaxMeshRatio = MinMeshRatio;
            }
        }

        void SetMaxMeshRatioWithAutoFix(float NewValue)
        {
            MaxMeshRatio = FMath::Clamp(NewValue, 0.0f, 1.0f);
            if (MaxMeshRatio < MinMeshRatio)
            {
                MinMeshRatio = MaxMeshRatio;
            }
        }

        TSharedRef<SWidget> BuildNumericRow(
            const FText& Label,
            TFunction<TOptional<float>()> Getter,
            float MinValue,
            float MaxValue,
            TFunction<void(float)> Setter)
        {
            return SNew(SHorizontalBox)
                + SHorizontalBox::Slot()
                .AutoWidth()
                .VAlign(VAlign_Center)
                [
                    SNew(STextBlock)
                    .Text(Label)
                ]
                + SHorizontalBox::Slot()
                .Padding(10, 0, 0, 0)
                .FillWidth(1.0f)
                [
                    SNew(SNumericEntryBox<float>)
                    .AllowSpin(true)
                    .MinValue(MinValue)
                    .MaxValue(MaxValue)
                    .Value_Lambda([Getter]() -> TOptional<float>
                    {
                        return Getter();
                    })
                    .OnValueChanged_Lambda([Setter](float NewValue)
                    {
                        Setter(NewValue);
                    })
                ];
        }

        void RefreshSceneTreeFromModule()
        {
            RootTreeItems.Reset();
            if (!Module)
            {
                if (SceneTreeView.IsValid())
                {
                    SceneTreeView->RequestTreeRefresh();
                }
                return;
            }

            TArray<FQEMSceneTreeNodeView> Snapshot;
            Module->GetSceneTreeSnapshot(Snapshot);

            TMap<int32, FSceneTreeItemPtr> ItemByNode;
            TMap<FString, FSceneTreeItemPtr> ItemByKey;
            for (const FQEMSceneTreeNodeView& NodeView : Snapshot)
            {
                FSceneTreeItemPtr Item = MakeShared<FSceneTreeItem>();
                Item->Node = NodeView;
                ItemByNode.Add(NodeView.NodeIndex, Item);
                if (!NodeView.NodeKey.IsEmpty())
                {
                    ItemByKey.Add(NodeView.NodeKey, Item);
                }
            }

            for (const FQEMSceneTreeNodeView& NodeView : Snapshot)
            {
                const FSceneTreeItemPtr* CurrentItem = ItemByNode.Find(NodeView.NodeIndex);
                if (!CurrentItem)
                {
                    continue;
                }

                const FSceneTreeItemPtr* ParentItem = ItemByNode.Find(NodeView.ParentIndex);
                if (ParentItem && ParentItem->IsValid())
                {
                    (*ParentItem)->Children.Add(*CurrentItem);
                }
                else
                {
                    RootTreeItems.Add(*CurrentItem);
                }
            }

            if (SceneTreeView.IsValid())
            {
                SceneTreeView->RequestTreeRefresh();
                for (const FSceneTreeItemPtr& RootItem : RootTreeItems)
                {
                    ExpandAll(RootItem);
                }

                if (!SelectedNodeKey.IsEmpty())
                {
                    if (const FSceneTreeItemPtr* SelectedItem = ItemByKey.Find(SelectedNodeKey))
                    {
                        SceneTreeView->SetSelection(*SelectedItem, ESelectInfo::Direct);
                    }
                    else
                    {
                        SelectedNodeKey.Empty();
                        SelectedNodeDisplayName.Empty();
                    }
                }
            }
        }

        void ExpandAll(const FSceneTreeItemPtr& Item)
        {
            if (!SceneTreeView.IsValid() || !Item.IsValid())
            {
                return;
            }

            SceneTreeView->SetItemExpansion(Item, true);
            for (const FSceneTreeItemPtr& Child : Item->Children)
            {
                ExpandAll(Child);
            }
        }

        TSharedRef<ITableRow> OnGenerateSceneTreeRow(FSceneTreeItemPtr Item, const TSharedRef<STableViewBase>& OwnerTable) const
        {
            FString RowText = TEXT("(无数据)");
            if (Item.IsValid())
            {
                const FString LimitText = Item->Node.bHasLimit
                    ? FString::Printf(TEXT(" | 限制<=%.1f%%"), static_cast<double>(Item->Node.LimitKeepRatio) * 100.0)
                    : TEXT("");

                RowText = FString::Printf(
                    TEXT("%s | 预设保留 %.1f%% | %u -> %u tris%s"),
                    *Item->Node.DisplayName,
                    static_cast<double>(Item->Node.KeepRatio) * 100.0,
                    Item->Node.SourceTriangles,
                    Item->Node.TargetTriangles,
                    *LimitText);
            }

            return SNew(STableRow<FSceneTreeItemPtr>, OwnerTable)
                [
                    SNew(STextBlock)
                    .Text(FText::FromString(RowText))
                    .AutoWrapText(true)
                ];
        }

        void OnGetSceneTreeChildren(FSceneTreeItemPtr Item, TArray<FSceneTreeItemPtr>& OutChildren) const
        {
            if (Item.IsValid())
            {
                OutChildren.Append(Item->Children);
            }
        }

        void OnSceneTreeSelectionChanged(FSceneTreeItemPtr Item, ESelectInfo::Type)
        {
            SelectedNodeKey.Empty();
            SelectedNodeDisplayName.Empty();

            if (!Item.IsValid())
            {
                return;
            }

            SelectedNodeKey = Item->Node.NodeKey;
            SelectedNodeDisplayName = Item->Node.DisplayName;

            float ExistingLimit = 1.0f;
            if (Module && Module->GetNodeKeepRatioLimit(SelectedNodeKey, ExistingLimit))
            {
                SelectedNodeLimitRatio = ExistingLimit;
            }
            else
            {
                SelectedNodeLimitRatio = Item->Node.KeepRatio;
            }
        }

        EActiveTimerReturnType HandleAutoRefresh(double, float)
        {
            RefreshSceneTreeFromModule();
            return EActiveTimerReturnType::Continue;
        }

    private:
        FQEMLevelSceneSimplifierModule* Module = nullptr;

        TSharedPtr<SEditableTextBox> DllPathTextBox;
        TSharedPtr<SNumericEntryBox<float>> SelectedNodeLimitEntry;
        TSharedPtr<STreeView<FSceneTreeItemPtr>> SceneTreeView;
        TArray<FSceneTreeItemPtr> RootTreeItems;

        FString SelectedNodeKey;
        FString SelectedNodeDisplayName;
        float SelectedNodeLimitRatio = 0.5f;

        float TargetRatio = 0.5f;
        float MinMeshRatio = 0.05f;
        float MaxMeshRatio = 1.0f;
        bool bOnlySelectedActors = false;
    };
}

const FName FQEMLevelSceneSimplifierModule::TabName(TEXT("QEMLevelSceneSimplifierTab"));

void FQEMLevelSceneSimplifierModule::StartupModule()
{
    ClearComputedPlan();

    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Module] Startup begin."));

    FString DllInitError;
    if (!EnsureQemApiLoaded(FString(), DllInitError))
    {
        UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Module] Startup DLL preload failed: %s"), *DllInitError);
        SetStatus(FString::Printf(TEXT("DLL 预加载失败：%s"), *DllInitError));
    }
    else
    {
        UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Module] Startup DLL preload succeeded: %s"), *GLoadedQemDllPath);
        SetStatus(FString::Printf(TEXT("就绪（DLL=%s）"), *GLoadedQemDllPath));
    }

    FGlobalTabmanager::Get()->RegisterNomadTabSpawner(
        TabName,
        FOnSpawnTab::CreateRaw(this, &FQEMLevelSceneSimplifierModule::SpawnTab))
        .SetDisplayName(LOCTEXT("TabDisplayName", "QEM 场景简化"))
        .SetMenuType(ETabSpawnerMenuType::Hidden);

    UToolMenus::RegisterStartupCallback(FSimpleMulticastDelegate::FDelegate::CreateRaw(this, &FQEMLevelSceneSimplifierModule::RegisterMenus));
    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Module] Startup complete."));
}

void FQEMLevelSceneSimplifierModule::ShutdownModule()
{
    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Module] Shutdown begin."));
    UToolMenus::UnRegisterStartupCallback(this);
    UToolMenus::UnregisterOwner(this);

    FGlobalTabmanager::Get()->UnregisterNomadTabSpawner(TabName);
    ClearComputedPlan();

    if (GQemRuntimeDllHandle != nullptr)
    {
        FPlatformProcess::FreeDllHandle(GQemRuntimeDllHandle);
        GQemRuntimeDllHandle = nullptr;
    }

    bQemApiReady = false;
    GLoadedQemDllPath.Empty();
    GFnQemGetAbiVersion = nullptr;
    GFnQemSceneGraphComputeDecisions = nullptr;

    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Module] Shutdown complete."));
}

void FQEMLevelSceneSimplifierModule::RegisterMenus()
{
    FToolMenuOwnerScoped OwnerScoped(this);

    UToolMenu* WindowMenu = UToolMenus::Get()->ExtendMenu("LevelEditor.MainMenu.Window");
    FToolMenuSection& Section = WindowMenu->AddSection("QEMLevelSceneSimplifier", LOCTEXT("QEMSection", "QEM"));

    Section.AddMenuEntry(
        "OpenQEMLevelSceneSimplifier",
        LOCTEXT("OpenMenuLabel", "QEM 场景简化器"),
        LOCTEXT("OpenMenuTooltip", "打开 QEM 关卡场景简化面板"),
        FSlateIcon(),
        FUIAction(FExecuteAction::CreateLambda([this]()
        {
            FGlobalTabmanager::Get()->TryInvokeTab(TabName);
        })));
}

TSharedRef<SDockTab> FQEMLevelSceneSimplifierModule::SpawnTab(const FSpawnTabArgs& SpawnTabArgs)
{
    return SNew(SDockTab)
        .TabRole(ETabRole::NomadTab)
        [
            SNew(SQEMLevelSceneSimplifierPanel)
            .Module(this)
        ];
}

void FQEMLevelSceneSimplifierModule::SetStatus(const FString& InStatus)
{
    FScopeLock Lock(&StatusMutex);
    StatusText = InStatus;
}

FString FQEMLevelSceneSimplifierModule::GetStatusText() const
{
    FScopeLock Lock(&StatusMutex);
    return StatusText;
}

bool FQEMLevelSceneSimplifierModule::HasComputedPlan() const
{
    FScopeLock Lock(&PlanMutex);
    return CachedPlan.IsValid();
}

void FQEMLevelSceneSimplifierModule::RebuildSceneTreeSnapshotLocked()
{
    uint64 TargetTriangles = 0;
    if (CachedPlan.IsValid())
    {
        BuildSceneTreeSnapshot(
            CachedPlan->Capture,
            &CachedPlan->Decisions,
            CachedPlan->DecisionCount,
            NodeKeepRatioLimits,
            SceneTreeSnapshot,
            TargetTriangles);
        return;
    }

    if (LastScannedCapture.IsValid())
    {
        BuildSceneTreeSnapshot(
            *LastScannedCapture,
            nullptr,
            0,
            NodeKeepRatioLimits,
            SceneTreeSnapshot,
            TargetTriangles);
        return;
    }

    SceneTreeSnapshot.Reset();
}

void FQEMLevelSceneSimplifierModule::GetSceneTreeSnapshot(TArray<FQEMSceneTreeNodeView>& OutNodes) const
{
    FScopeLock Lock(&PlanMutex);
    OutNodes = SceneTreeSnapshot;
}

void FQEMLevelSceneSimplifierModule::ClearComputedPlan()
{
    if (!IsInGameThread())
    {
        RunOnGameThreadSync([this]()
        {
            ClearComputedPlan();
        });
        return;
    }

    FScopeLock Lock(&PlanMutex);
    UE_LOG(LogQEMLevelSceneSimplifier, Verbose, TEXT("[Plan] Clear cached simplify plan."));
    CachedPlan.Reset();
    RebuildSceneTreeSnapshotLocked();
}

void FQEMLevelSceneSimplifierModule::ScanCurrentLevelNodes(bool bOnlySelectedActors)
{
    UE_LOG(
        LogQEMLevelSceneSimplifier,
        Log,
        TEXT("[Scene] Scan requested. Scope=%s"),
        bOnlySelectedActors ? TEXT("SelectedActors") : TEXT("WholeLevel"));

    if (bIsRunning.Exchange(true))
    {
        SetStatus(TEXT("已有任务在执行中，请稍候..."));
        return;
    }

    ProgressValue.Store(0.0f);
    SetStatus(bOnlySelectedActors ? TEXT("扫描任务已提交：选中 Actor 节点（后台）...") : TEXT("扫描任务已提交：关卡全部节点（后台）..."));

    Async(EAsyncExecution::ThreadPool, [this, bOnlySelectedActors]()
    {
        FLevelSceneCapture Capture;
        FString CaptureError;
        bool bCaptureSucceeded = false;
        const bool bCaptureDispatched = RunOnGameThreadSync([&]()
        {
            bCaptureSucceeded = CaptureCurrentLevelScene(Capture, bOnlySelectedActors, CaptureError);
        });

        if (!bCaptureDispatched)
        {
            ProgressValue.Store(0.0f);
            SetStatus(TEXT("扫描失败：无法切换到主线程执行场景采集"));
            bIsRunning.Store(false);
            return;
        }

        if (!bCaptureSucceeded)
        {
            ProgressValue.Store(0.0f);
            SetStatus(FString::Printf(TEXT("扫描失败：%s"), *CaptureError));
            bIsRunning.Store(false);
            return;
        }

        const int32 NodeCount = Capture.Nodes.Num();
        const int32 MeshCount = Capture.Meshes.Num();

        const bool bStoreDispatched = RunOnGameThreadSync([this, CapturedScene = MoveTemp(Capture)]() mutable
        {
            FScopeLock Lock(&PlanMutex);
            CachedPlan.Reset();
            LastScannedCapture = MakeShared<FLevelSceneCapture, ESPMode::ThreadSafe>(MoveTemp(CapturedScene));
            RebuildSceneTreeSnapshotLocked();
        });

        if (!bStoreDispatched)
        {
            ProgressValue.Store(0.0f);
            SetStatus(TEXT("扫描失败：无法切换到主线程更新场景树"));
            bIsRunning.Store(false);
            return;
        }

        ProgressValue.Store(1.0f);
        SetStatus(FString::Printf(TEXT("扫描完成：节点=%d，唯一网格=%d。可在树中选择节点并设置限制。"), NodeCount, MeshCount));
        bIsRunning.Store(false);
    });
}

void FQEMLevelSceneSimplifierModule::SetNodeKeepRatioLimit(const FString& NodeKey, float MaxKeepRatio)
{
    if (NodeKey.IsEmpty())
    {
        return;
    }

    const float Clamped = FMath::Clamp(MaxKeepRatio, 0.0f, 1.0f);
    {
        FScopeLock Lock(&PlanMutex);
        NodeKeepRatioLimits.Add(NodeKey, Clamped);
        RebuildSceneTreeSnapshotLocked();
    }

    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Limit] Set node limit. node=%s, max_keep_ratio=%.3f"), *NodeKey, Clamped);
}

void FQEMLevelSceneSimplifierModule::ClearNodeKeepRatioLimit(const FString& NodeKey)
{
    if (NodeKey.IsEmpty())
    {
        return;
    }

    {
        FScopeLock Lock(&PlanMutex);
        NodeKeepRatioLimits.Remove(NodeKey);
        RebuildSceneTreeSnapshotLocked();
    }

    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Limit] Cleared node limit. node=%s"), *NodeKey);
}

void FQEMLevelSceneSimplifierModule::ClearAllNodeKeepRatioLimits()
{
    {
        FScopeLock Lock(&PlanMutex);
        NodeKeepRatioLimits.Reset();
        RebuildSceneTreeSnapshotLocked();
    }

    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Limit] Cleared all node limits."));
}

bool FQEMLevelSceneSimplifierModule::GetNodeKeepRatioLimit(const FString& NodeKey, float& OutMaxKeepRatio) const
{
    OutMaxKeepRatio = 1.0f;
    if (NodeKey.IsEmpty())
    {
        return false;
    }

    FScopeLock Lock(&PlanMutex);
    if (const float* Found = NodeKeepRatioLimits.Find(NodeKey))
    {
        OutMaxKeepRatio = *Found;
        return true;
    }

    return false;
}

void FQEMLevelSceneSimplifierModule::ComputeSimplifyParameters(
    float TargetRatio,
    float MinMeshRatio,
    float MaxMeshRatio,
    bool bOnlySelectedActors,
    const FString& DllOverridePath)
{
    UE_LOG(
        LogQEMLevelSceneSimplifier,
        Log,
        TEXT("[Plan] Compute requested. target=%.3f, min=%.3f, max=%.3f, selected_only=%d, override=%s"),
        TargetRatio,
        MinMeshRatio,
        MaxMeshRatio,
        bOnlySelectedActors ? 1 : 0,
        DllOverridePath.IsEmpty() ? TEXT("<empty>") : *DllOverridePath);

    if (bIsRunning.Exchange(true))
    {
        UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Plan] Compute rejected: another task is running."));
        SetStatus(TEXT("已有任务在执行中，请稍候..."));
        return;
    }

    ProgressValue.Store(0.0f);
    SetStatus(bOnlySelectedActors ? TEXT("参数计算任务已提交：选中 Actor（后台）...") : TEXT("参数计算任务已提交：整个关卡（后台）..."));

    Async(EAsyncExecution::ThreadPool, [this, TargetRatio, MinMeshRatio, MaxMeshRatio, bOnlySelectedActors, DllOverridePath]()
    {
        FLevelSceneCapture Capture;
        FString CaptureError;
        bool bCaptureSucceeded = false;
        const bool bCaptureDispatched = RunOnGameThreadSync([&]()
        {
            bCaptureSucceeded = CaptureCurrentLevelScene(Capture, bOnlySelectedActors, CaptureError);
        });

        if (!bCaptureDispatched)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Compute failed: cannot dispatch capture to game thread."));
            ClearComputedPlan();
            bIsRunning.Store(false);
            SetStatus(TEXT("参数计算失败：无法切换到主线程执行场景采集"));
            return;
        }

        if (!bCaptureSucceeded)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Compute failed at capture: %s"), *CaptureError);
            ClearComputedPlan();
            bIsRunning.Store(false);
            SetStatus(FString::Printf(TEXT("参数计算失败：%s"), *CaptureError));
            return;
        }

        FString DllError;
        if (!EnsureQemApiLoaded(DllOverridePath, DllError))
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Compute failed at DLL load: %s"), *DllError);
            ClearComputedPlan();
            bIsRunning.Store(false);
            SetStatus(FString::Printf(TEXT("参数计算失败（DLL 决策计算不可用）：%s"), *DllError));
            return;
        }

        TArray<QemSceneMeshDecision> Decisions;
        uint32 DecisionCount = 0;
        QemSceneSimplifyResult SceneResult{};
        FString DecisionError;
        if (!ComputeSceneDecisionsWithDll(
            Capture,
            TargetRatio,
            MinMeshRatio,
            MaxMeshRatio,
            Decisions,
            DecisionCount,
            SceneResult,
            DecisionError))
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Compute failed at decision stage: %s"), *DecisionError);
            ClearComputedPlan();
            bIsRunning.Store(false);
            SetStatus(FString::Printf(TEXT("参数计算失败（DLL 决策计算失败）：%s"), *DecisionError));
            return;
        }

        TMap<FString, float> NodeLimitsSnapshot;
        {
            FScopeLock Lock(&PlanMutex);
            NodeLimitsSnapshot = NodeKeepRatioLimits;
        }

        const int32 LimitedMeshes = ApplyNodeLimitsToDecisions(Capture, NodeLimitsSnapshot, Decisions, DecisionCount);
        if (LimitedMeshes > 0)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Limit] Applied node limits during compute. limited_meshes=%d"), LimitedMeshes);
        }

        const FString PreviewSummary = BuildPreviewSummary(
            Capture,
            Decisions,
            DecisionCount,
            TargetRatio,
            MinMeshRatio,
            MaxMeshRatio,
            bOnlySelectedActors);

        uint64 TargetTriangles = 0;
        if (Capture.Meshes.Num() > 0)
        {
            TArray<const QemSceneMeshDecision*> DecisionsByMesh;
            DecisionsByMesh.Init(nullptr, Capture.Meshes.Num());
            for (uint32 Index = 0; Index < DecisionCount && Index < static_cast<uint32>(Decisions.Num()); ++Index)
            {
                const QemSceneMeshDecision& Decision = Decisions[Index];
                if (Decision.mesh_index < static_cast<uint32>(Capture.Meshes.Num()))
                {
                    DecisionsByMesh[Decision.mesh_index] = &Decision;
                }
            }

            for (const FLevelSceneNodeData& Node : Capture.Nodes)
            {
                if (Node.MeshIndex < 0 || Node.MeshIndex >= Capture.Meshes.Num())
                {
                    continue;
                }

                const uint32 SourceTriangles = static_cast<uint32>(Capture.Meshes[Node.MeshIndex].Indices.Num() / 3);
                uint32 NodeTargetTriangles = SourceTriangles;
                if (const QemSceneMeshDecision* Decision = DecisionsByMesh[Node.MeshIndex])
                {
                    if (SourceTriangles >= 2)
                    {
                        NodeTargetTriangles = FMath::Clamp(Decision->target_triangles, 2u, SourceTriangles);
                    }
                    else
                    {
                        NodeTargetTriangles = Decision->target_triangles;
                    }
                }

                TargetTriangles += NodeTargetTriangles;
            }
        }

        TSharedPtr<FComputedSimplifyPlan, ESPMode::ThreadSafe> NewPlan = MakeShared<FComputedSimplifyPlan, ESPMode::ThreadSafe>();
        NewPlan->Capture = MoveTemp(Capture);
        NewPlan->Decisions = MoveTemp(Decisions);
        NewPlan->DecisionCount = DecisionCount;
        NewPlan->SourceTriangleCount = SceneResult.source_triangles;
        NewPlan->TargetTriangleCount = TargetTriangles;
        NewPlan->bOnlySelectedActors = bOnlySelectedActors;
        NewPlan->TargetRatio = FMath::Clamp(TargetRatio, 0.01f, 1.0f);
        NewPlan->MinMeshRatio = FMath::Clamp(MinMeshRatio, 0.0f, 1.0f);
        NewPlan->MaxMeshRatio = FMath::Clamp(MaxMeshRatio, NewPlan->MinMeshRatio, 1.0f);

        int32 CachedNodeCount = 0;
        const bool bStoreDispatched = RunOnGameThreadSync([this, NewPlanLocal = MoveTemp(NewPlan), &CachedNodeCount]() mutable
        {
            FScopeLock Lock(&PlanMutex);
            CachedPlan = MoveTemp(NewPlanLocal);
            LastScannedCapture = MakeShared<FLevelSceneCapture, ESPMode::ThreadSafe>(CachedPlan->Capture);
            RebuildSceneTreeSnapshotLocked();
            CachedNodeCount = SceneTreeSnapshot.Num();
        });

        if (!bStoreDispatched)
        {
            UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Compute failed: cannot dispatch plan storage to game thread."));
            ClearComputedPlan();
            bIsRunning.Store(false);
            SetStatus(TEXT("参数计算失败：无法切换到主线程更新缓存结果"));
            return;
        }

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Plan] Compute success. source=%llu, target=%llu, decisions=%u, cached_nodes=%d"),
            SceneResult.source_triangles,
            TargetTriangles,
            DecisionCount,
            CachedNodeCount);

        ProgressValue.Store(1.0f);
        SetStatus(FString::Printf(
            TEXT("%s\n参数已缓存，可直接点击“2) 执行简化（使用已计算参数）”。"),
            *PreviewSummary));
        bIsRunning.Store(false);
    });
}

void FQEMLevelSceneSimplifierModule::ApplyComputedSimplification()
{
    UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Plan] Apply requested."));

    if (bIsRunning.Exchange(true))
    {
        UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Plan] Apply rejected: another task is running."));
        SetStatus(TEXT("已有任务在执行中，请稍候..."));
        return;
    }

    TSharedPtr<FComputedSimplifyPlan, ESPMode::ThreadSafe> Plan;
    TMap<FString, float> NodeLimitsSnapshot;
    {
        FScopeLock Lock(&PlanMutex);
        Plan = CachedPlan;
        NodeLimitsSnapshot = NodeKeepRatioLimits;
    }

    if (!Plan.IsValid())
    {
        UE_LOG(LogQEMLevelSceneSimplifier, Warning, TEXT("[Plan] Apply rejected: no cached plan."));
        bIsRunning.Store(false);
        SetStatus(TEXT("尚未计算参数，请先点击“1) 计算简化参数（不写回）”。"));
        return;
    }

    UE_LOG(
        LogQEMLevelSceneSimplifier,
        Log,
        TEXT("[Plan] Apply begin. cached_source=%llu, cached_target=%llu, decisions=%u, selected_only=%d"),
        Plan->SourceTriangleCount,
        Plan->TargetTriangleCount,
        Plan->DecisionCount,
        Plan->bOnlySelectedActors ? 1 : 0);

    ProgressValue.Store(0.0f);
    SetStatus(Plan->bOnlySelectedActors
        ? TEXT("执行任务已提交：仅选中 Actor（后台）...")
        : TEXT("执行任务已提交：整个关卡（后台）..."));

    Async(EAsyncExecution::ThreadPool, [this, Plan, NodeLimitsSnapshot]()
    {
        const FStageProgressBridge BackupProgressBridge {
            &ProgressValue,
            &StatusMutex,
            &StatusText,
            0.05f,
            0.45f,
        };

        const FStageProgressBridge ApplyProgressBridge {
            &ProgressValue,
            &StatusMutex,
            &StatusText,
            0.45f,
            0.95f,
        };

        FBackupResult BackupResult;
        int32 LimitedMeshes = 0;
        FString ApplyError;
        int32 UpdatedMeshCount = 0;
        uint64 OutputTriangles = 0;
        FString ReducerVersion;
        bool bApplySucceeded = false;
        FString FailureStatus;

        const bool bApplyDispatched = RunOnGameThreadSync([&]()
        {
            FString BackupError;
            if (!BackupMeshesBeforeApply(Plan->Capture, BackupResult, BackupError, &BackupProgressBridge))
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Apply failed at backup: %s"), *BackupError);
                FailureStatus = FString::Printf(TEXT("备份失败，已终止写回：%s"), *BackupError);
                return;
            }

            SetStatus(FString::Printf(
                TEXT("备份完成：%d/%d，目录=%s；开始写回（使用缓存参数）..."),
                BackupResult.Succeeded,
                BackupResult.Attempted,
                *BackupResult.BackupFolder));

            TArray<QemSceneMeshDecision> EffectiveDecisions = Plan->Decisions;
            LimitedMeshes = ApplyNodeLimitsToDecisions(Plan->Capture, NodeLimitsSnapshot, EffectiveDecisions, Plan->DecisionCount);
            if (LimitedMeshes > 0)
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Log, TEXT("[Limit] Applied node limits during apply. limited_meshes=%d"), LimitedMeshes);
            }

            if (!ApplySimplifiedMeshesWithMeshReduction(
                    Plan->Capture,
                    EffectiveDecisions,
                    Plan->DecisionCount,
                    UpdatedMeshCount,
                    OutputTriangles,
                    ReducerVersion,
                    ApplyError,
                    &ApplyProgressBridge))
            {
                UE_LOG(LogQEMLevelSceneSimplifier, Error, TEXT("[Plan] Apply failed at mesh reduction: %s"), *ApplyError);
                FailureStatus = FString::Printf(TEXT("写回失败：%s"), *ApplyError);
                return;
            }

            bApplySucceeded = true;
        });

        if (!bApplyDispatched)
        {
            SetStatus(TEXT("执行失败：无法切换到主线程执行备份/写回"));
            ProgressValue.Store(0.0f);
            bIsRunning.Store(false);
            return;
        }

        if (!bApplySucceeded)
        {
            SetStatus(FailureStatus.IsEmpty() ? TEXT("执行失败：未知错误") : FailureStatus);
            ProgressValue.Store(0.0f);
            bIsRunning.Store(false);
            return;
        }

        const uint64 SourceTriangleCount = (Plan->SourceTriangleCount > 0)
            ? Plan->SourceTriangleCount
            : CountSourceTriangles(Plan->Capture);

        const double ReductionRatio = (SourceTriangleCount > 0)
            ? (1.0 - static_cast<double>(OutputTriangles) / static_cast<double>(SourceTriangleCount))
            : 0.0;

        ProgressValue.Store(1.0f);
        SetStatus(FString::Printf(
            TEXT("完成：已备份 %d 个网格到 %s；MeshReduction=%s；写回 %d 个网格资产，三角形 %llu -> %llu（减少 %.1f%%）。"),
            BackupResult.Succeeded,
            *BackupResult.BackupFolder,
            *ReducerVersion,
            UpdatedMeshCount,
            SourceTriangleCount,
            OutputTriangles,
            FMath::Clamp(ReductionRatio * 100.0, 0.0, 100.0)));

        UE_LOG(
            LogQEMLevelSceneSimplifier,
            Log,
            TEXT("[Plan] Apply success. updated_meshes=%d, source=%llu, output=%llu, reduction=%.2f%%"),
            UpdatedMeshCount,
            SourceTriangleCount,
            OutputTriangles,
            FMath::Clamp(ReductionRatio * 100.0, 0.0, 100.0));

        bIsRunning.Store(false);
    });
}

IMPLEMENT_MODULE(FQEMLevelSceneSimplifierModule, QEMLevelSceneSimplifier)

#undef LOCTEXT_NAMESPACE
