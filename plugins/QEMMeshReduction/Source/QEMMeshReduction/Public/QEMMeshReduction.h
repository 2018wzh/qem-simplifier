#pragma once

#include "Modules/ModuleManager.h"
#include "IMeshReductionInterfaces.h"

class FQEMMeshReductionModule final : public IMeshReductionModule
{
public:
    virtual void StartupModule() override;
    virtual void ShutdownModule() override;

    virtual IMeshReduction* GetStaticMeshReductionInterface() override;
    virtual IMeshReduction* GetSkeletalMeshReductionInterface() override;
    virtual IMeshMerging* GetMeshMergingInterface() override;
    virtual IMeshMerging* GetDistributedMeshMergingInterface() override;
    virtual FString GetName() override;
};
