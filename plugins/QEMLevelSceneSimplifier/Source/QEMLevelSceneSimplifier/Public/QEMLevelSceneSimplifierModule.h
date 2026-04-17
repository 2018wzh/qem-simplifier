#pragma once

#include "CoreMinimal.h"
#include "Modules/ModuleManager.h"

class SDockTab;
struct FComputedSimplifyPlan;
struct FLevelSceneCapture;

struct FQEMSceneTreeNodeView
{
    int32 NodeIndex = INDEX_NONE;
    int32 ParentIndex = INDEX_NONE;
    FString NodeKey;
    FString DisplayName;
    uint32 SourceTriangles = 0;
    uint32 TargetTriangles = 0;
    float KeepRatio = 1.0f;
    bool bHasLimit = false;
    float LimitKeepRatio = 1.0f;
};

class FQEMLevelSceneSimplifierModule : public IModuleInterface
{
public:
    virtual void StartupModule() override;
    virtual void ShutdownModule() override;

    void ComputeSimplifyParameters(float TargetRatio, float MinMeshRatio, float MaxMeshRatio, bool bOnlySelectedActors, const FString& DllOverridePath);
    void ApplyComputedSimplification();

    void ScanCurrentLevelNodes(bool bOnlySelectedActors);
    void SetNodeKeepRatioLimit(const FString& NodeKey, float MaxKeepRatio);
    void ClearNodeKeepRatioLimit(const FString& NodeKey);
    void ClearAllNodeKeepRatioLimits();
    bool GetNodeKeepRatioLimit(const FString& NodeKey, float& OutMaxKeepRatio) const;

    bool HasComputedPlan() const;
    void GetSceneTreeSnapshot(TArray<FQEMSceneTreeNodeView>& OutNodes) const;

    bool IsRunning() const { return bIsRunning; }
    float GetProgress() const { return ProgressValue.Load(); }
    FString GetStatusText() const;

private:
    TSharedRef<SDockTab> SpawnTab(const class FSpawnTabArgs& SpawnTabArgs);
    void RegisterMenus();
    void ClearComputedPlan();
    void RebuildSceneTreeSnapshotLocked();

private:
    static const FName TabName;

    TAtomic<bool> bIsRunning { false };
    TAtomic<float> ProgressValue { 0.0f };

    mutable FCriticalSection StatusMutex;
    FString StatusText;

    mutable FCriticalSection PlanMutex;
    TSharedPtr<FComputedSimplifyPlan, ESPMode::ThreadSafe> CachedPlan;
    TSharedPtr<FLevelSceneCapture, ESPMode::ThreadSafe> LastScannedCapture;
    TMap<FString, float> NodeKeepRatioLimits;
    TArray<FQEMSceneTreeNodeView> SceneTreeSnapshot;

    void SetStatus(const FString& InStatus);
};
